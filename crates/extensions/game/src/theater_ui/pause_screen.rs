//! The Theater's pause menu as the game draws it (docs/game/theater.md,
//! "The pause menu"): our list class on the `theater.pause-menu` tag, and
//! the kit's value picker over it, for a film whose picture is the guest's.
//!
//! The host keeps the menu: the film's guest reads neutral pads
//! (`theater::neutral_pads`), so no press reaches a guest screen, and the
//! pad and the keyboard both step the host's model
//! (`playback_page::Menu`). This module shows that model with the game's
//! widgets. Each UI update the driver compares what the model wants
//! ([`Model`]) with what is up ([`Mirror`]) and takes one step: open the
//! menu, put its cursor on the model's row, open the picker over it, move
//! the picker's cursor, or close either. A row of the menu never acts on a
//! guest press ([`Rows::press`] is nothing).
//!
//! A film with no guest picture (a dedicated host's) and a film player
//! process, which installs no screen, keep the host's page.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use guest_layout::entry::{Cx, EntryError, GuestCall, Ret};
use guest_layout::memory::{GuestMemory, MemFault};
use guest_layout::points;
use hooks::Flow;

use super::pages::{GuestTexts, SPARE, TEXTS};
use super::{PAUSE_MENU, Theater};
use crate::list_class::{self, Action, ListClass, LiveText, OnFree, Rows};
use crate::ui::{Chosen, LABEL_SLOT};
use crate::ui_kit::input::footer_legend;
use crate::ui_kit::picker::{self, Pick};
use crate::ui_kit::spec::ScreenSpec;
use crate::ui_kit::dialog_tag;

/// The class's feature and the driver's.
pub(super) const CLASS: &str = "theater-pause-menu";
pub(super) const DRIVER: &str = "theater-pause-driver";

/// The menu's title when the model's is cleared for a picker over it.
const TITLE_TEXT: &str = "THEATER";

/// The menu's text slots in the Theater's family, after the library's and
/// the film page's: the title, the footer, then a line a row.
const BASE: u32 = 0xc0;
const TITLE: u32 = BASE;
const FOOTER: u32 = BASE + 1;
const ROWS: u32 = BASE + 2;
/// The most rows the slots hold.
pub(super) const MAX_ROWS: usize = 16;
const _: () = assert!(SPARE <= BASE && ROWS + MAX_ROWS as u32 <= TEXTS.len);

/// The local player the screens open for: player one.
const PLAYER_ONE: u32 = 1;
/// UI updates a picker may take to build its pane before the driver gives
/// it up (about two seconds).
const PICK_WAIT: u32 = 120;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A value picker the menu's model has open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct PickModel {
    pub title: String,
    pub values: Vec<String>,
    pub focus: usize,
    pub help: String,
}

impl PickModel {
    /// What makes it another picker: a change of either is a new dialog.
    fn same_dialog(&self, other: &PickModel) -> bool {
        self.title == other.title && self.values == other.values
    }
}

/// The pause menu as the game is to show it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Model {
    pub title: String,
    /// A line a row: its label, with its value after a colon.
    pub rows: Vec<String>,
    /// The footer legend with each row focused.
    pub legends: Vec<String>,
    pub focus: usize,
    pub pick: Option<PickModel>,
}

/// `spec` (`playback_page::Menu::spec`) as the guest's model: the menu's
/// rows, and the picker when the spec is one over the menu.
pub(super) fn model(spec: &ScreenSpec) -> Model {
    let (menu, pick) = match (&spec.dialog, &spec.underlay) {
        (Some(_), Some(under)) => (
            under.as_ref(),
            Some(PickModel {
                title: spec.title.clone(),
                values: spec.rows.iter().map(|r| r.label.clone()).collect(),
                focus: spec.focus,
                help: spec
                    .rows
                    .get(spec.focus)
                    .and_then(|r| r.description.clone())
                    .unwrap_or_default(),
            }),
        ),
        _ => (spec, None),
    };
    Model {
        title: if menu.title.is_empty() { TITLE_TEXT.into() } else { menu.title.clone() },
        rows: menu.rows.iter().take(MAX_ROWS).map(|r| r.cells(menu.style).0).collect(),
        legends: menu
            .rows
            .iter()
            .take(MAX_ROWS)
            .map(|r| footer_legend(r.prompts.as_deref().unwrap_or(menu.prompts.as_slice())))
            .collect(),
        focus: menu.focus.min(menu.rows.len().saturating_sub(1)),
        pick,
    }
}

/// What the driver does at one UI update.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Open the menu's screen.
    OpenMenu,
    /// Close the menu's screen.
    CloseMenu(u32),
    /// Put the menu's cursor on a row.
    FocusMenu(u32, usize),
    /// Offer the pick and open the kit's picker over the menu.
    OpenPick(PickModel),
    ClosePick(u32),
    FocusPick(u32, usize),
}

/// The picker the driver opened.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PickUp {
    model: PickModel,
    /// Its pane was built: the kit reported its screen.
    seen: bool,
    closing: bool,
    /// UI updates since it was asked for and not yet seen.
    waited: u32,
}

/// What of the menu is up in the guest.
#[derive(Debug, Default)]
pub(super) struct Mirror {
    /// The open was asked for and the pane is not built yet.
    requested: bool,
    closing: bool,
    /// The row the menu's cursor was last put on.
    focus: usize,
    pick: Option<PickUp>,
}

impl Mirror {
    /// The menu's screen was freed: nothing of it is up.
    fn freed(&mut self) {
        *self = Mirror::default();
    }

    /// The next step toward `want` (`None`: no menu), with the menu's
    /// screen `menu` (once its pane is built) and the kit picker's
    /// `pick_screen` (while one is up). One step an update; `None` when
    /// what is up is what is wanted, or a step is still in the air.
    pub(super) fn step(
        &mut self,
        want: Option<&Model>,
        menu: Option<u32>,
        pick_screen: Option<u32>,
    ) -> Option<Step> {
        if menu.is_some() {
            self.requested = false;
        }
        // The picker first: it is over the menu, and closes before it.
        if let Some(up) = &mut self.pick {
            up.seen |= pick_screen.is_some();
            if up.seen && pick_screen.is_none() {
                self.pick = None;
                return None;
            }
            let wanted = want
                .and_then(|m| m.pick.as_ref())
                .filter(|p| p.same_dialog(&up.model));
            match (wanted, pick_screen) {
                (_, None) => {
                    up.waited += 1;
                    if up.waited > PICK_WAIT {
                        eprintln!("theater: the pause menu's picker did not open; the menu shows its row");
                        self.pick = None;
                    }
                    return None;
                }
                (None, Some(screen)) => {
                    if up.closing {
                        return None;
                    }
                    up.closing = true;
                    return Some(Step::ClosePick(screen));
                }
                (Some(p), Some(screen)) => {
                    if up.closing || up.model.focus == p.focus {
                        return None;
                    }
                    up.model.focus = p.focus;
                    return Some(Step::FocusPick(screen, p.focus));
                }
            }
        }
        let Some(want) = want else {
            return match menu {
                Some(screen) if !self.closing => {
                    self.closing = true;
                    Some(Step::CloseMenu(screen))
                }
                _ => None,
            };
        };
        let Some(screen) = menu else {
            if self.requested {
                return None;
            }
            self.requested = true;
            self.closing = false;
            self.focus = want.focus;
            return Some(Step::OpenMenu);
        };
        if self.closing {
            // It closes and is freed first; then it opens again.
            return None;
        }
        if let Some(p) = &want.pick {
            self.pick = Some(PickUp {
                model: p.clone(),
                seen: false,
                closing: false,
                waited: 0,
            });
            return Some(Step::OpenPick(p.clone()));
        }
        if self.focus != want.focus {
            self.focus = want.focus;
            return Some(Step::FocusMenu(screen, want.focus));
        }
        None
    }
}

/// The screen's state between the class's callbacks and the driver.
struct Screen {
    theater: Theater,
    texts: Arc<GuestTexts>,
    /// The model the screen last showed.
    model: Mutex<Model>,
    /// The rows last drawn, for the live redraw.
    drawn: Mutex<Vec<String>>,
    mirror: Mutex<Mirror>,
    /// The menu's screen, once its pane is built (0: none).
    screen: AtomicU32,
    /// The row to put the cursor on once the pane is built.
    opening_focus: AtomicU32,
}

impl Screen {
    /// The model now, kept for the callbacks that come after the host's
    /// menu has gone (the screen draws while it closes).
    fn refresh(&self) -> Model {
        let wanted = self.theater.pause_model();
        let mut held = lock(&self.model);
        if let Some(model) = wanted {
            *held = model;
        }
        held.clone()
    }

    fn up(&self) -> Option<u32> {
        Some(self.screen.load(Ordering::SeqCst)).filter(|s| *s != 0)
    }
}

impl Rows for Screen {
    fn title(&self) -> u32 {
        let model = self.refresh();
        self.texts.set(TITLE, &model.title)
    }

    fn count(&self) -> usize {
        self.refresh().rows.len()
    }

    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let model = lock(&self.model);
        let Some((i, line)) = usize::try_from(row)
            .ok()
            .and_then(|i| model.rows.get(i).map(|line| (i, line)))
        else {
            return Vec::new();
        };
        vec![(LABEL_SLOT, self.texts.set(ROWS + i as u32, line))]
    }

    fn line(&self, row: i16) -> Option<String> {
        lock(&self.model).rows.get(usize::try_from(row).ok()?).cloned()
    }

    /// The film's guest reads neutral pads; a press that reaches the row
    /// some other way (a debug pad) does nothing: the host's menu acts.
    fn press(&self, _row: i16) -> Chosen {
        Chosen::Nothing
    }

    fn wraps(&self) -> bool {
        false
    }

    fn focused_footer(&self, row: Option<i16>) -> Option<u32> {
        let model = lock(&self.model);
        let row = row.and_then(|r| usize::try_from(r).ok()).unwrap_or(model.focus);
        let legend = model.legends.get(row).or_else(|| model.legends.first())?;
        Some(self.texts.set(FOOTER, legend))
    }

    fn live_text(&self) -> LiveText {
        let rows = self.refresh().rows;
        let mut drawn = lock(&self.drawn);
        let redraw = *drawn != rows;
        if redraw {
            *drawn = rows;
        }
        LiveText { writes: Vec::new(), redraw }
    }

    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        self.texts.write(m)
    }
}

fn class(screen: &Arc<Screen>) -> ListClass {
    let me = Arc::clone(screen);
    let ready: Action = Arc::new(move |cx, at| {
        me.screen.store(at, Ordering::SeqCst);
        let row = me.opening_focus.swap(u32::MAX, Ordering::SeqCst);
        Box::pin(async move {
            if row != u32::MAX {
                set_row(&cx, at, row as usize).await?;
            }
            Ok(())
        })
    });
    let me = Arc::clone(screen);
    let freed: OnFree = Arc::new(move |_, at| {
        // Another opening may have started since: only this screen's free
        // (or one before any pane) ends it.
        let ours = me.screen.load(Ordering::SeqCst);
        if ours == 0 || ours == at {
            me.screen.store(0, Ordering::SeqCst);
            lock(&me.mirror).freed();
        }
        Box::pin(async { Ok(()) })
    });
    let footer = screen.texts.set(FOOTER, "");
    ListClass::with_rows(CLASS, PAUSE_MENU, Arc::clone(screen) as Arc<dyn Rows>)
        .layout(super::playback_page::menu_layout())
        .lines()
        .action_buttons(&[])
        .pane_ready(ready)
        .on_free(freed)
        .footer(footer)
}

async fn set_row(cx: &Cx, screen: u32, row: usize) -> Result<(), EntryError> {
    let list = screen.wrapping_add(guest_layout::view::ui::LIST);
    cx.call(GuestCall::new(list_class::LIST_SET_ROW).eax(list).args([row as u32]))
        .await?;
    Ok(())
}

/// The kit's pick for `p`: its setter does nothing, the host's menu
/// chooses (`playback_page::Menu::press`).
fn pick_of(p: &PickModel) -> Pick {
    Pick::new(&p.title, p.values.clone(), p.focus, &p.help, Arc::new(|i| format!("theater: value {i} is the host menu's to set")))
}

async fn take(cx: &Cx, me: &Screen, step: Step) -> Result<(), EntryError> {
    match step {
        Step::OpenMenu => {
            let Some(loader) = list_class::loader_of(PAUSE_MENU) else {
                return Ok(());
            };
            cx.call(GuestCall::new(points::OPEN_SCREEN.base).args([PLAYER_ONE, loader]))
                .await?;
        }
        Step::CloseMenu(screen) | Step::ClosePick(screen) => dialog_tag::close(cx, screen).await?,
        Step::FocusMenu(screen, row) | Step::FocusPick(screen, row) => set_row(cx, screen, row).await?,
        Step::OpenPick(p) => {
            let opened = match picker::offer_list(pick_of(&p))
                .map(|name| picker::screen_for(name, true))
                .and_then(list_class::loader_of)
                .and_then(dialog_tag::by_loader)
            {
                Some(d) => dialog_tag::open_over_screen(cx, &d, PLAYER_ONE).await? != 0,
                None => false,
            };
            if !opened {
                eprintln!("theater: the game could not make the pause menu's picker");
                lock(&me.mirror).pick = None;
            }
        }
    }
    Ok(())
}

/// The driver's call of the UI update is passing through its own hook.
static PASSING: AtomicBool = AtomicBool::new(false);

struct Driver(Arc<Screen>);

impl hooks::Feature for Driver {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(DRIVER, "Show the Theater's pause menu with the game's screens")
            .doc("docs/game/theater.md")
            .presentation_only()
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // The menu is the host's; the mirror is rebuilt from it.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let me = Arc::clone(&self.0);
        b.raw()
            .before(&points::UI_UPDATE, "mirror the Theater pause menu", move |c| {
                if PASSING.load(Ordering::SeqCst) {
                    return Ok(Flow::Continue);
                }
                let want = me.theater.pause_model();
                let menu = me.up();
                if want.is_none() && menu.is_none() {
                    return Ok(Flow::Continue);
                }
                if let Some(model) = &want {
                    *lock(&me.model) = model.clone();
                }
                let Some(step) = lock(&me.mirror).step(want.as_ref(), menu, picker::screen()) else {
                    return Ok(Flow::Continue);
                };
                if step == Step::OpenMenu {
                    let focus = want.as_ref().map_or(0, |m| m.focus);
                    me.opening_focus.store(focus as u32, Ordering::SeqCst);
                }
                let me = Arc::clone(&me);
                c.answer_async(move |cx| {
                    Box::pin(async move {
                        let elapsed = cx.arg(0)?;
                        if let Err(e) = take(&cx, &me, step.clone()).await {
                            eprintln!("theater: the pause menu's {step:?}: {e}");
                        }
                        PASSING.store(true, Ordering::SeqCst);
                        let out = cx
                            .call(GuestCall::new(points::UI_UPDATE.base).args([elapsed]))
                            .await;
                        PASSING.store(false, Ordering::SeqCst);
                        Ok(Ret::new(out?.eax, 4))
                    })
                })
            })
            // Inside the other UI update drivers, as theirs are.
            .after_feature("live-join-link")
            .after_feature("theater-link")
            .after_feature("forge-screens")
            .after_feature("lab-takeover")
            .after_feature(crate::pause_change_map::FEATURE)
            .after_feature(crate::variant_quick::page::ROW_FEATURE)
            .optional();
        Ok(())
    }
}

/// Whether the game can show the pause menu: its class is built.
pub(super) fn installed() -> bool {
    list_class::loader_of(PAUSE_MENU).is_some()
}

/// Adds the pause menu's class and its driver. With the class switched
/// off the driver is left out and the host draws the menu.
pub(super) fn install(
    registry: &mut hooks::Registry,
    theater: &Theater,
    texts: &Arc<GuestTexts>,
) -> anyhow::Result<()> {
    texts.set(TITLE, TITLE_TEXT);
    let screen = Arc::new(Screen {
        theater: theater.clone(),
        texts: Arc::clone(texts),
        model: Mutex::default(),
        drawn: Mutex::default(),
        mirror: Mutex::default(),
        screen: AtomicU32::new(0),
        opening_focus: AtomicU32::new(u32::MAX),
    });
    if registry.add(&class(&screen)).map_err(anyhow::Error::msg)? {
        registry.add(&Driver(screen)).map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theater_ui::overlay::Track;
    use crate::theater_ui::playback_page::Menu;
    use crate::ui_kit::input::Input;

    fn track() -> Track {
        use crate::theater_ui::adapter::{FilmMeta, Player, Source};
        let mut track = Track::from_entry(&crate::theater_ui::Entry {
            film: FilmMeta {
                id: "test".into(),
                map: "lockout".into(),
                length_ticks: 3000,
                source: Source::Local,
                ..FilmMeta::default()
            },
            clip: None,
            archive_clip: None,
        });
        track.players = ["Chief", "Marathon"]
            .into_iter()
            .map(|name| Player { name: name.into(), ..Player::default() })
            .collect();
        track
    }

    fn menu_model(menu: &Menu, track: &Track) -> Model {
        model(&menu.spec(track, None))
    }

    #[test]
    fn the_model_is_the_host_menu_s_rows_and_its_picker() {
        let track = track();
        let mut menu = Menu::default();
        let m = menu_model(&menu, &track);
        assert_eq!(m.title, "THEATER");
        assert_eq!(m.rows.len(), 10);
        assert_eq!(m.rows[0], "Resume");
        assert!(m.rows[2].starts_with("Change Player"), "{:?}", m.rows[2]);
        assert_eq!(m.rows[9], "End Film");
        assert_eq!(m.legends.len(), m.rows.len());
        assert_eq!((m.focus, m.pick.as_ref()), (0, None));

        // A on Change Player opens its picker; the menu stays under it.
        menu.press(&track, Input::Down);
        menu.press(&track, Input::Down);
        assert_eq!(menu_model(&menu, &track).focus, 2);
        menu.press(&track, Input::A);
        let m = menu_model(&menu, &track);
        let pick = m.pick.clone().expect("a picker over the menu");
        assert_eq!(pick.values, ["Chief", "Marathon"]);
        assert_eq!(pick.focus, 0);
        assert_eq!(pick.help, "Choose the camera's subject.");
        assert_eq!(m.title, "THEATER", "the menu under the picker keeps its title");
        assert_eq!((m.rows.len(), m.focus), (10, 2));

        menu.press(&track, Input::Down);
        assert_eq!(menu_model(&menu, &track).pick.unwrap().focus, 1);
        menu.press(&track, Input::B);
        assert!(menu_model(&menu, &track).pick.is_none(), "B closes the picker");
    }

    #[test]
    fn a_row_with_nothing_to_choose_has_its_own_legend() {
        let mut track = track();
        track.players.clear();
        let m = menu_model(&Menu::default(), &track);
        assert_ne!(m.legends[2], m.legends[0], "Change Player with no players takes no A");
    }

    const MENU: u32 = 0x1000;
    const PICK: u32 = 0x2000;

    #[test]
    fn the_mirror_opens_the_menu_once_and_follows_its_cursor() {
        let track = track();
        let mut menu = Menu::default();
        let mut mirror = Mirror::default();
        let want = menu_model(&menu, &track);
        assert_eq!(mirror.step(Some(&want), None, None), Some(Step::OpenMenu));
        assert_eq!(mirror.step(Some(&want), None, None), None, "the pane is not built yet");
        assert_eq!(mirror.step(Some(&want), Some(MENU), None), None, "up, at the opening row");

        menu.press(&track, Input::Down);
        let want = menu_model(&menu, &track);
        assert_eq!(mirror.step(Some(&want), Some(MENU), None), Some(Step::FocusMenu(MENU, 1)));
        assert_eq!(mirror.step(Some(&want), Some(MENU), None), None);

        // Resume: the host's menu goes, the screen closes once and is freed.
        assert_eq!(mirror.step(None, Some(MENU), None), Some(Step::CloseMenu(MENU)));
        assert_eq!(mirror.step(None, Some(MENU), None), None, "closing");
        mirror.freed();
        assert_eq!(mirror.step(None, None, None), None);
        // Start again opens it again.
        assert_eq!(mirror.step(Some(&want), None, None), Some(Step::OpenMenu));
    }

    #[test]
    fn the_mirror_opens_the_picker_over_the_menu_and_closes_it_first() {
        let track = track();
        let mut menu = Menu::default();
        let mut mirror = Mirror::default();
        mirror.step(Some(&menu_model(&menu, &track)), None, None);
        menu.press(&track, Input::Down);
        menu.press(&track, Input::Down);
        let want = menu_model(&menu, &track);
        assert_eq!(mirror.step(Some(&want), Some(MENU), None), Some(Step::FocusMenu(MENU, 2)));

        menu.press(&track, Input::A);
        let want = menu_model(&menu, &track);
        let pick = want.pick.clone().unwrap();
        assert_eq!(mirror.step(Some(&want), Some(MENU), None), Some(Step::OpenPick(pick)));
        assert_eq!(mirror.step(Some(&want), Some(MENU), None), None, "its pane is not built yet");
        assert_eq!(mirror.step(Some(&want), Some(MENU), Some(PICK)), None, "up, at the held value");

        menu.press(&track, Input::Down);
        let want = menu_model(&menu, &track);
        assert_eq!(mirror.step(Some(&want), Some(MENU), Some(PICK)), Some(Step::FocusPick(PICK, 1)));

        // A chooses in the host's menu: the picker closes, the menu stays.
        assert!(menu.press(&track, Input::A).is_some());
        let want = menu_model(&menu, &track);
        assert!(want.pick.is_none());
        assert_eq!(mirror.step(Some(&want), Some(MENU), Some(PICK)), Some(Step::ClosePick(PICK)));
        assert_eq!(mirror.step(Some(&want), Some(MENU), Some(PICK)), None, "closing");
        assert_eq!(mirror.step(Some(&want), Some(MENU), None), None, "freed");
        assert_eq!(mirror.step(Some(&want), Some(MENU), None), None, "the menu is as wanted");

        // The menu going with a picker up closes the picker, then the menu.
        menu.press(&track, Input::A);
        let want = menu_model(&menu, &track);
        assert!(matches!(mirror.step(Some(&want), Some(MENU), None), Some(Step::OpenPick(_))));
        mirror.step(Some(&want), Some(MENU), Some(PICK));
        assert_eq!(mirror.step(None, Some(MENU), Some(PICK)), Some(Step::ClosePick(PICK)));
        assert_eq!(mirror.step(None, Some(MENU), None), None, "freed");
        assert_eq!(mirror.step(None, Some(MENU), None), Some(Step::CloseMenu(MENU)));
    }

    #[test]
    fn a_picker_that_never_opens_is_given_up() {
        let track = track();
        let mut menu = Menu::default();
        let mut mirror = Mirror::default();
        mirror.step(Some(&menu_model(&menu, &track)), None, None);
        for _ in 0..3 {
            menu.press(&track, Input::Down);
        }
        let at_camera = menu_model(&menu, &track);
        mirror.step(Some(&at_camera), Some(MENU), None);
        menu.press(&track, Input::A);
        let want = menu_model(&menu, &track);
        assert!(matches!(mirror.step(Some(&want), Some(MENU), None), Some(Step::OpenPick(_))));
        // The host's menu leaves the picker while the guest's never came.
        menu.press(&track, Input::B);
        let want = menu_model(&menu, &track);
        for _ in 0..PICK_WAIT {
            assert_eq!(mirror.step(Some(&want), Some(MENU), None), None);
        }
        assert_eq!(mirror.step(Some(&want), Some(MENU), None), None, "given up");
        assert!(mirror.pick.is_none());
    }

    #[test]
    fn the_menu_s_slots_are_past_the_library_s_and_inside_the_family() {
        assert!(BASE >= SPARE);
        assert!(ROWS + MAX_ROWS as u32 <= TEXTS.len);
        assert!(MAX_ROWS >= 10, "the menu's ten rows");
    }
}
