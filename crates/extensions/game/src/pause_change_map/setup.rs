//! Game Setup's screens on the pause menu: Game Setup itself, the map
//! categories, the game types, Quick Options and Clanker Options, each a
//! list class of ours built on screen 200 in a game; and the value picker
//! (`crate::ui_kit::picker`, the large dialog in a game) for the maps, the
//! variants, the Quick Options values and the Clankers' settings
//! (docs/game/settings-menu.md "Game Setup from the pause menu").
//!
//! - A on Change Map opens the categories (or one category's maps
//!   straight away in the picker); A on a category opens its maps in the
//!   picker. Picking one closes the picker and the categories, back on
//!   Game Setup. Change Rules is the same with the game types and their
//!   variants.
//! - B on Game Setup ([`back`]): with nothing in the draft it closes; with
//!   a draft it opens the picker over **Restart with these settings** and
//!   **Keep playing** (Keep playing focused). Restart ends the game
//!   (`super::commit`); Keep playing drops the draft and closes Game
//!   Setup; B on the dialog goes back to Game Setup with the draft kept.

use std::sync::Arc;

use engine::entry::{Cx, EntryError, GuestCall};
use engine::memory::{GuestMemory, MemFault};
use engine::points;

use crate::list_class::{self, Action, ListClass, Rows};
use crate::map_select::Category;
use crate::ui::Chosen;
use crate::ui_kit::picker::{self, Pick};
use crate::ui_kit::{dialog_tag, lifecycle};

use super::clanker_rows::ClankerRows;
use super::game_types::{self, Group};
use super::quick::{self, QuickRow};
use super::*;

/// B and Back's button words (`list_class::BUTTON_EVENT`): Game Setup takes
/// them as its action, to ask before leaving with a draft.
pub const B_BUTTON: u32 = 1;
pub const BACK_BUTTON: u32 = 13;
static BACK_BUTTONS: [u32; 2] = [B_BUTTON, BACK_BUTTON];

const MAP_HELP: &str = "The map the next game is played on.";
const RULES_HELP: &str = "The game variant the next game plays.";
/// The confirm dialog's title, as the picker puts it in capitals.
pub const CONFIRM_LABEL: &str = "Restart the game";

type ActionFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), EntryError>> + Send>>;

fn offer(p: Option<Pick>) -> Chosen {
    p.and_then(picker::offer_list).map_or(Chosen::Nothing, Chosen::Open)
}

/// Opens the value picker for controller `port`, from an action with no
/// press to take the player from: as `list_class::open` opens it.
async fn open_picker(cx: &Cx, port: u32) -> Result<(), EntryError> {
    let ingame = cx.read(list_class::ingame).unwrap_or(true);
    let name = picker::screen_for(picker::NAME, ingame);
    let Some(loader) = list_class::loader_of(name) else {
        eprintln!("pause game setup: {name} is off; not opened");
        return Ok(());
    };
    let mask = 1u32 << port.min(3);
    if let Some(d) = dialog_tag::by_loader(loader) {
        return dialog_tag::open_over(cx, &d, mask).await;
    }
    cx.call(GuestCall::new(points::OPEN_SCREEN.base).args([mask, loader]))
        .await?;
    Ok(())
}

/// Closes the categories or game types list a picker was opened from.
fn close_list(visit: &Arc<Visit>) -> Action {
    let v = Arc::clone(visit);
    Arc::new(move |cx: Cx, _picker: u32| -> ActionFuture {
        let v = Arc::clone(&v);
        Box::pin(async move {
            if let Some(screen) = v.with(|s| s.list_screen.take()) {
                lifecycle::close(&cx, screen).await?;
            }
            Ok(())
        })
    })
}

/// The picker over category `c`'s maps; `from_list` when the categories
/// list is under it, to close once a map is picked.
pub fn map_pick(visit: &Arc<Visit>, c: Category, from_list: bool) -> Option<Pick> {
    let maps = visit.with(|s| super::maps(&s.maps, c));
    if maps.is_empty() {
        return None;
    }
    let values: Vec<String> = maps.iter().map(|m| m.name.clone()).collect();
    let held = visit.with(|s| s.draft.map.as_ref().map(|(id, _)| *id));
    let current = held
        .and_then(|id| maps.iter().position(|m| m.id == id))
        .unwrap_or(0);
    let v = Arc::clone(visit);
    let set = Arc::new(move |i: usize| match maps.get(i) {
        Some(m) => {
            // With Clankers on, the map's nav mesh is asked for now, as
            // Game Setup's Change Map does (docs/game/bots.md §10 stage 2).
            crate::clankers::map_picked(&m.short_name());
            v.with(|s| s.draft.pick_map(m.id, &m.name));
            format!("pause game setup: the next game's map is {} ({:#x})", m.name, m.id)
        }
        None => format!("pause game setup: no map {i}"),
    });
    let pick = Pick::new(c.label(), values, current, MAP_HELP, set);
    Some(if from_list { pick.after(close_list(visit)) } else { pick })
}

/// The picker over `group`'s variants; the game types list is under it.
pub fn rules_pick(visit: &Arc<Visit>, group: Group) -> Option<Pick> {
    let list = visit.with(|s| game_types::of_group(&s.rules, group));
    if list.is_empty() {
        return None;
    }
    let values: Vec<String> = list.iter().map(|l| l.name.clone()).collect();
    let held = visit.with(|s| s.draft.rules.clone());
    let current = held
        .and_then(|n| list.iter().position(|l| l.name == n))
        .unwrap_or(0);
    let v = Arc::clone(visit);
    let set = Arc::new(move |i: usize| match list.get(i) {
        Some(l) => {
            let mode = crate::lobby_start::mode();
            let maps = catalogue().for_variant(mode, Some(&l.variant));
            let map = super::opening(crate::map_select::variant_mode(mode, Some(&l.variant)), &maps);
            v.with(|s| {
                s.draft.pick_rules(l);
                s.maps = maps;
                s.avail.map = map;
            });
            format!("pause game setup: the next game plays {}", l.name)
        }
        None => format!("pause game setup: no variant {i}"),
    });
    Some(Pick::new(&group.label(), values, current, RULES_HELP, set).after(close_list(visit)))
}

/// The picker over a Quick Options row's values.
pub fn quick_pick(visit: &Arc<Visit>, row: QuickRow) -> Pick {
    let values = row.texts();
    let (label, help, current) = (row.setting.label(), row.setting.help(), row.current);
    let v = Arc::clone(visit);
    let set = Arc::new(move |i: usize| {
        let Some(&value) = row.values.get(i) else {
            return format!("pause game setup: no value {i}");
        };
        let done = v.with(|s| {
            let base = s.base.clone();
            s.draft.set_quick(base.as_ref(), row.setting, row.id, value)
        });
        match done {
            Ok(line) => format!("pause game setup: Quick Options: {line}"),
            Err(why) => format!("pause game setup: Quick Options: {label} not changed: {why}"),
        }
    });
    Pick::new(label, values, current, help, set)
}

/// The confirm dialog for `draft`.
pub fn confirm_pick(visit: &Arc<Visit>, draft: &Draft) -> Pick {
    let v = Arc::clone(visit);
    let set = Arc::new(move |i: usize| {
        let c = Confirm::ALL.get(i).copied();
        v.with(|s| s.confirm = c);
        format!("pause game setup: {}", c.map_or("nothing", Confirm::label))
    });
    let values = Confirm::ALL.iter().map(|c| c.label().to_owned()).collect();
    let keep = Confirm::ALL
        .iter()
        .position(|c| *c == Confirm::KeepPlaying)
        .unwrap_or(0);
    Pick::new(CONFIRM_LABEL, values, keep, &draft.summary(), set).after(confirmed(visit))
}

/// What the confirm dialog's choice does once it has closed.
fn confirmed(visit: &Arc<Visit>) -> Action {
    let v = Arc::clone(visit);
    Arc::new(move |cx: Cx, _picker: u32| -> ActionFuture {
        let v = Arc::clone(&v);
        Box::pin(async move {
            match v.with(|s| s.confirm.take()) {
                Some(Confirm::Restart) => super::commit(&cx, &v).await,
                Some(Confirm::KeepPlaying) => {
                    let screen = v.with(|s| {
                        s.draft = Draft::default();
                        s.setup_screen.take()
                    });
                    eprintln!("pause game setup: keep playing; the draft is dropped");
                    if let Some(screen) = screen {
                        lifecycle::close(&cx, screen).await?;
                    }
                }
                None => {}
            }
            Ok(())
        })
    })
}

/// B and Back on Game Setup.
pub fn back(visit: &Arc<Visit>) -> Action {
    let v = Arc::clone(visit);
    Arc::new(move |cx: Cx, screen: u32| -> ActionFuture {
        let v = Arc::clone(&v);
        Box::pin(async move {
            let draft = v.draft();
            if draft.is_empty() {
                return lifecycle::close(&cx, screen).await;
            }
            v.with(|s| {
                s.setup_screen = Some(screen);
                s.confirm = None;
            });
            if picker::offer_list(confirm_pick(&v, &draft)).is_some() {
                open_picker(&cx, v.port()).await?;
            }
            Ok(())
        })
    })
}

/// Game Setup's rows.
pub struct SetupRows(pub Arc<Visit>);

impl SetupRows {
    fn row(&self, row: i16) -> Option<SetupRow> {
        let a = self.0.with(|s| s.avail);
        usize::try_from(row).ok().and_then(|r| setup_rows(a).get(r).copied())
    }
}

impl Rows for SetupRows {
    fn title(&self) -> u32 {
        IDS.id(SETUP_TITLE_SLOT)
    }
    fn count(&self) -> usize {
        TEXTS.set(SETUP_TITLE_SLOT, SETUP_TITLE);
        setup_rows(self.0.with(|s| s.avail)).len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        match self.line(row) {
            Some(l) => vec![(0, TEXTS.set(SETUP_SLOTS + row as u32, &l))],
            None => Vec::new(),
        }
    }
    fn line(&self, row: i16) -> Option<String> {
        let r = self.row(row)?;
        Some(self.0.with(|s| r.line(&s.draft)))
    }
    fn press(&self, row: i16) -> Chosen {
        let Some(r) = self.row(row) else {
            return Chosen::Nothing;
        };
        match r {
            SetupRow::ChangeMap => match self.0.with(|s| s.avail.map) {
                Some(Opening::Categories) => Chosen::Push(CATEGORIES_SCREEN),
                Some(Opening::Maps(c)) => offer(map_pick(&self.0, c, false)),
                None => Chosen::Nothing,
            },
            SetupRow::ChangeRules => Chosen::Push(TYPES_SCREEN),
            SetupRow::QuickOptions => Chosen::Push(QUICK_SCREEN),
            SetupRow::ClankerOptions => Chosen::Push(CLANKERS_SCREEN),
        }
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        TEXTS.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let lines: Vec<String> = (0..count as i16).filter_map(|r| self.line(r)).collect();
        Some(format!("{SETUP_TITLE}: {count} rows ({})", lines.join(", ")))
    }
}

fn category_slot(c: Category) -> u32 {
    CATEGORY_SLOTS + Category::ALL.iter().position(|k| *k == c).unwrap_or(0) as u32
}

/// The categories' screen: A on one opens its maps in the picker.
pub struct CategoryRows(pub Arc<Visit>);

impl Rows for CategoryRows {
    fn title(&self) -> u32 {
        IDS.id(CATEGORIES_TITLE_SLOT)
    }
    fn count(&self) -> usize {
        TEXTS.set(CATEGORIES_TITLE_SLOT, CATEGORIES_TITLE);
        self.0.with(|s| categories(&s.maps)).len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let cats = self.0.with(|s| categories(&s.maps));
        match usize::try_from(row).ok().and_then(|r| cats.get(r)) {
            Some(c) => vec![(0, TEXTS.set(category_slot(*c), c.label()))],
            None => Vec::new(),
        }
    }
    fn press(&self, row: i16) -> Chosen {
        let cats = self.0.with(|s| categories(&s.maps));
        match usize::try_from(row).ok().and_then(|r| cats.get(r)) {
            Some(&c) => offer(map_pick(&self.0, c, true)),
            None => Chosen::Nothing,
        }
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        TEXTS.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let labels: Vec<&str> = self.0.with(|s| categories(&s.maps)).iter().map(|c| c.label()).collect();
        Some(format!("{CATEGORIES_TITLE}: {count} rows ({})", labels.join(", ")))
    }
}

/// The game types' screen: A on one opens its variants in the picker.
pub struct TypeRows(pub Arc<Visit>);

impl TypeRows {
    fn groups(&self) -> Vec<Group> {
        self.0.with(|s| game_types::groups(&s.rules))
    }
}

impl Rows for TypeRows {
    fn title(&self) -> u32 {
        IDS.id(TYPES_TITLE_SLOT)
    }
    fn count(&self) -> usize {
        TEXTS.set(TYPES_TITLE_SLOT, TYPES_TITLE);
        self.groups().len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        match usize::try_from(row).ok().and_then(|r| self.groups().get(r).copied()) {
            Some(g) => vec![(0, TEXTS.set(TYPE_SLOTS + g.index() as u32, &g.label()))],
            None => Vec::new(),
        }
    }
    fn press(&self, row: i16) -> Chosen {
        match usize::try_from(row).ok().and_then(|r| self.groups().get(r).copied()) {
            Some(g) => offer(rules_pick(&self.0, g)),
            None => Chosen::Nothing,
        }
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        TEXTS.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let labels: Vec<String> = self.groups().iter().map(|g| g.label()).collect();
        Some(format!("{TYPES_TITLE}: {count} rows ({})", labels.join(", ")))
    }
}

/// Quick Options' screen: the working variant's rows ([`quick::rows`]).
pub struct QuickRows(pub Arc<Visit>);

impl QuickRows {
    fn rows(&self) -> Vec<QuickRow> {
        let offered = crate::community_variants::last_offered();
        self.0.with(|s| {
            s.draft
                .working(s.base.as_ref())
                .map_or_else(Vec::new, |v| quick::rows(v, offered.as_ref()))
        })
    }
    fn at(&self, row: i16) -> Option<QuickRow> {
        usize::try_from(row).ok().and_then(|r| self.rows().into_iter().nth(r))
    }
}

impl Rows for QuickRows {
    fn title(&self) -> u32 {
        IDS.id(QUICK_TITLE_SLOT)
    }
    fn count(&self) -> usize {
        TEXTS.set(QUICK_TITLE_SLOT, QUICK_TITLE);
        self.rows().len()
    }
    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        match self.at(row) {
            Some(r) => vec![(0, TEXTS.set(QUICK_SLOTS + row as u32, &r.line()))],
            None => Vec::new(),
        }
    }
    fn line(&self, row: i16) -> Option<String> {
        self.at(row).map(|r| r.line())
    }
    fn press(&self, row: i16) -> Chosen {
        match self.at(row) {
            Some(r) => offer(Some(quick_pick(&self.0, r))),
            None => Chosen::Nothing,
        }
    }
    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        TEXTS.write(m)
    }
    fn describe(&self, count: usize) -> Option<String> {
        let lines: Vec<String> = self.rows().iter().map(QuickRow::line).collect();
        Some(format!("{QUICK_TITLE}: {count} rows ({})", lines.join(", ")))
    }
}

/// Notes the list a picker opens from, so the pick can close it.
fn note_list(visit: &Arc<Visit>) -> Action {
    let v = Arc::clone(visit);
    Arc::new(move |_cx: Cx, screen: u32| -> ActionFuture {
        v.with(|s| s.list_screen = Some(screen));
        Box::pin(async { Ok(()) })
    })
}

/// Forgets that list once the stack frees it.
fn forget_list(visit: &Arc<Visit>) -> Action {
    let v = Arc::clone(visit);
    Arc::new(move |_cx: Cx, screen: u32| -> ActionFuture {
        v.with(|s| {
            if s.list_screen == Some(screen) {
                s.list_screen = None;
            }
        });
        Box::pin(async { Ok(()) })
    })
}

/// Game Setup first, then the categories, the game types, Quick Options
/// and, with `clankers`, Clanker Options. Each opens in a game, on the
/// in-game Controller Settings' tag (`crate::list_class`, module docs);
/// the ones whose rows change draw one line a row.
pub fn classes(visit: &Arc<Visit>, clankers: Option<Arc<settings::Store>>) -> Vec<ListClass> {
    let setup = ListClass::with_rows(SETUP_SCREEN, SETUP_SCREEN, Arc::new(SetupRows(Arc::clone(visit))))
        .lines()
        .action(back(visit))
        .action_buttons(&BACK_BUTTONS);
    let categories = ListClass::with_rows(
        CATEGORIES_SCREEN,
        CATEGORIES_SCREEN,
        Arc::new(CategoryRows(Arc::clone(visit))),
    )
    .pane_ready(note_list(visit))
    .on_free(forget_list(visit));
    let types = ListClass::with_rows(TYPES_SCREEN, TYPES_SCREEN, Arc::new(TypeRows(Arc::clone(visit))))
        .pane_ready(note_list(visit))
        .on_free(forget_list(visit));
    let quick = ListClass::with_rows(QUICK_SCREEN, QUICK_SCREEN, Arc::new(QuickRows(Arc::clone(visit))))
        .lines();
    let mut out = vec![setup, categories, types, quick];
    if let Some(store) = clankers {
        out.push(
            ListClass::with_rows(CLANKERS_SCREEN, CLANKERS_SCREEN, Arc::new(ClankerRows(store)))
                .lines(),
        );
    }
    out
}
