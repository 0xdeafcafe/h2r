use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};
use runtime::guest::Permissions;

use super::selector::{Filter, Sort, fitted, map_stem, map_title};
use super::{DETAIL, Entry, FILMS, Load, Tab, Theater, adapter::FilmSummary};
use crate::list_class::{
    ListClass, ListLayout, LiveText, Panel, PanelText, PictureSpec, Rows, TextBlock,
};
use crate::mods_screen::page_layout as lobby;
use crate::ui::Chosen;
use crate::ui_kit::input::{Glyph, Input, Prompt, legend};
use crate::ui_kit::spec::{ListPlace, PaneText, Press, RowSpec, ScreenSpec, ValueStyle};
use crate::ui_kit::view::{Art, PaneArt};

pub(super) const TEXTS: crate::ui_kit::ids::Family = crate::ui_kit::ids::family("theater");
const CAPACITY: u32 = 512;
const FILMS_TITLE: u32 = 1;
const DETAIL_TITLE: u32 = 2;
const FILMS_FOOTER: u32 = 3;
const DETAIL_FOOTER: u32 = 4;
const STATUS: u32 = 5;
const ROWS: u32 = 8;
const ROSTER: usize = 8;
pub(super) const FILM_ROWS: usize = 40;
pub(super) const BROWSE_ROWS: usize = 4;
const MAX_ROWS: usize = FILM_ROWS + BROWSE_ROWS;
const DETAIL_ROWS: u32 = ROWS + MAX_ROWS as u32;
const TAB_PANEL: u32 = DETAIL_ROWS + 4;
const PANEL: u32 = TAB_PANEL + 1;

const FILMS_LAYOUT: ListLayout = crate::skulls_menu::LIST_LAYOUT;
// The right column has its own band for tabs above the preview.
const TAB_STRIP: TextBlock = TextBlock {
    rect: [176, 39, 152, 260],
    ..lobby::BODY
};
const LIST_PICTURE: PictureSpec = PictureSpec {
    at: [TAB_STRIP.rect[1], TAB_STRIP.rect[2] - 4],
    animation: crate::campaign_menu::PICTURE.animation,
    ..lobby::PICTURE
};
const FILM_LAYOUT: ListLayout = ListLayout {
    visible: 4,
    ..lobby::LIST
};
const FILM_PICTURE: PictureSpec = PictureSpec {
    at: [-258, 46],
    ..lobby::PICTURE
};
// The 128-pixel lobby display ends at y=50, just above the film picture.
const FILM_PANEL_ART: [PictureSpec; 2] = [
    lobby::NAME_FIELD,
    PictureSpec {
        at: [lobby::LOBBY_DISPLAY.at[0], 178],
        ..lobby::LOBBY_DISPLAY
    },
];
const LIST_STATUS: TextBlock = TextBlock {
    rect: [
        LIST_PICTURE.at[1] - lobby::PICTURE_SIZE[1] as i16 - 4,
        TAB_STRIP.rect[1],
        -174,
        TAB_STRIP.rect[3],
    ],
    ..crate::skulls_menu::DESCRIPTION_TEXT
};
const FILM_STATUS: TextBlock = TextBlock {
    rect: [66, -258, 47, 40],
    ..lobby::STATUS
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Watch,
    Save,
    Delete,
}

impl Action {
    fn for_entry(entry: &Entry) -> &'static [Self] {
        if entry.clip.is_some() {
            &[Self::Watch, Self::Delete]
        } else if entry.film.archive_id().is_some() {
            &[Self::Watch, Self::Save]
        } else {
            &[Self::Watch, Self::Save, Self::Delete]
        }
    }
}

impl Theater {
    fn choose_film(&self, row: i16) -> Chosen {
        let mut state = self.state();
        if row == 0 && state.library_error().is_some() {
            let (controller, player) = (state.controller, state.player.clone());
            drop(state);
            self.open(controller, player);
            return Chosen::Stepped("Refreshing films".into());
        }
        let Some(entry) = entry_at(&state, row) else {
            return Chosen::Nothing;
        };
        state.select(entry);
        Chosen::Push(DETAIL)
    }

    pub(crate) fn step_detail(&self, row: i16) -> bool {
        let mut state = self.state();
        let Some(entry) = state.selected.clone().filter(|_| !state.working) else {
            return false;
        };
        let Some(&action) = usize::try_from(row)
            .ok()
            .and_then(|i| Action::for_entry(&entry).get(i))
        else {
            return false;
        };
        if action != Action::Delete {
            state.confirm_delete = false;
        }
        state.status.clear();
        match action {
            Action::Watch if entry.film.watch_unavailable().is_none() => {
                if entry.film.archive_id().is_some() {
                    drop(state);
                    self.archive_action(Action::Watch, entry);
                    return false;
                }
                state.status = "Opening film...".into();
                state.changed();
                drop(state);
                if let Err(error) = self.session().play(&entry) {
                    let mut state = self.state();
                    state.status = error;
                    state.changed();
                }
                return false;
            }
            Action::Watch => {}
            Action::Delete if !state.confirm_delete => {
                state.confirm_delete = true;
                state.status = "Press A again to delete".into();
            }
            Action::Save | Action::Delete => {
                if (action == Action::Save && entry.film.archive_id().is_some())
                    || entry.archive_clip.is_some()
                {
                    drop(state);
                    self.archive_action(action, entry);
                    return false;
                }
                drop(state);
                let result = self.change_library(action, &entry);
                let mut state = self.state();
                state.confirm_delete = false;
                state.status = match result {
                    Ok(()) if action == Action::Delete => "Deleted".into(),
                    Ok(()) if entry.film.saved => "Removed from Saved".into(),
                    Ok(()) => "Added to Saved".into(),
                    Err(error) => error,
                };
                state.changed();
                return action == Action::Delete && state.selected.is_none();
            }
        }
        state.changed();
        false
    }
}

fn entry_at(state: &super::State, row: i16) -> Option<Entry> {
    let index = usize::try_from(row).ok()?.checked_sub(BROWSE_ROWS)?;
    state.entries().into_iter().nth(index)
}

fn tabs(state: &super::State) -> Vec<String> {
    super::Tab::ALL
        .into_iter()
        .map(|tab| {
            if state.tab == tab {
                format!("[{}]", tab.label())
            } else {
                tab.label().to_owned()
            }
        })
        .collect()
}

fn films_model(state: &super::State) -> Vec<RowSpec> {
    let mut tabs = RowSpec::info("Film library", "").press(Press::Step);
    if let Some(error) = state.library_error() {
        tabs = RowSpec::action("Retry library").describe(&fitted(error, 28));
    } else if let Some(error) = state
        .clips_error
        .as_ref()
        .filter(|_| state.tab == Tab::Clips)
    {
        tabs = tabs.describe(&fitted(error, 28));
    } else {
        tabs = tabs.describe("Choose Recent, Saved or Clips.");
    }
    let sort = [
        Sort::Recent.label(),
        Sort::Map.label(),
        Sort::GameType.label(),
    ];
    let mut rows = vec![
        tabs,
        RowSpec::value("Sort", &sort, state.selector.sort as usize)
            .describe("Choose the order of the films."),
        filter_row("Map", state.selector.map.as_deref()).describe("Show films from one level."),
        filter_row("Type", state.selector.game_type.as_deref())
            .describe("Show films of one game type."),
    ];
    let entries = state.entries();
    if entries.is_empty() {
        let label = match state.load {
            Load::Loading => "Reading films...",
            Load::Ready(_)
                if state.selector.map.is_some() || state.selector.game_type.is_some() =>
            {
                "No matching films"
            }
            Load::Ready(_) => match state.tab {
                Tab::Recent => "No recent films",
                Tab::Saved => "No saved films",
                Tab::Clips => "No clips",
            },
        };
        rows.push(RowSpec::info(label, ""));
    } else {
        rows.extend(entries.iter().map(|entry| {
            RowSpec::info(&entry.row_title(), "")
                .press(Press::Open(DETAIL.into()))
                .describe(&entry.browse_description(state.player.as_deref()))
        }));
    }
    rows
}

fn filter_row(label: &str, choice: Option<&str>) -> RowSpec {
    let value = fitted(choice.unwrap_or("All"), 15);
    RowSpec::info(label, &format!("< {value} >")).press(Press::Step)
}

fn detail_model(state: &super::State) -> Vec<RowSpec> {
    let Some(entry) = &state.selected else {
        return vec![RowSpec::info("Film unavailable", "")];
    };
    let film = &entry.film;
    Action::for_entry(entry)
        .iter()
        .copied()
        .map(|action| {
            let label = match action {
                Action::Watch => "Watch",
                Action::Save if film.saved => "Unsave",
                Action::Save => "Save",
                Action::Delete if state.confirm_delete => "Confirm delete",
                Action::Delete => "Delete",
            };
            if action == Action::Watch
                && let Some(reason) = film.watch_unavailable()
            {
                return RowSpec::info("Watch unavailable", "").describe(reason);
            }
            if state.working {
                RowSpec::info(label, "")
            } else {
                RowSpec::action(label)
            }
        })
        .collect()
}

fn prompts(list: bool) -> Vec<Prompt> {
    let mut prompts = vec![
        Prompt::new(&[Glyph::A], if list { "Details" } else { "Select" }),
        Prompt::new(&[Glyph::B], "Back"),
    ];
    prompts.push(Prompt::new(
        &[Glyph::DpadLeft, Glyph::DpadRight],
        if list { "Change" } else { "Players" },
    ));
    prompts
}

fn panel_cell(rect: [i16; 4], right: bool, data: bool) -> PanelText {
    PanelText {
        block: TextBlock {
            rect,
            colour: if data { lobby::PALE_BLUE } else { lobby::WHITE },
            ..lobby::BODY
        },
        right,
        bias: lobby::TEXT_BIAS,
    }
}

fn film_panel() -> Panel {
    let mut texts = vec![panel_cell([180, 52, 161, 272], false, false)];
    for i in 0..6 {
        let y = 159 - i * 19;
        texts.push(panel_cell([y, 52, y - 18, 272], false, true));
    }
    texts.push(panel_cell([44, -12, 25, 272], false, false));
    for row in 0..ROSTER as i16 {
        let y = 24 - row * 20;
        for (left, right, numeric) in [
            (-12, 110, false),
            (113, 172, false),
            (174, 211, true),
            (214, 272, true),
        ] {
            texts.push(panel_cell([y, left, y - 19, right], numeric, true));
        }
    }
    Panel {
        art: FILM_PANEL_ART.to_vec(),
        texts,
    }
}

fn film_panel_values(state: &super::State) -> Vec<String> {
    let Some(entry) = &state.selected else {
        return Vec::new();
    };
    let film = &entry.film;
    let variant = film.variant_name();
    let variant = if variant.is_empty() { "--" } else { variant };
    let roster_pages = film.scoreboard().len().div_ceil(ROSTER).max(1);
    let mut values = vec![
        film.source_badge().into(),
        format!("Mode   {}", fitted(film.mode().unwrap_or("--"), 21)),
        format!("Type   {}", fitted(film.game_type().unwrap_or("--"), 21)),
        format!("Variant   {}", fitted(variant, 19)),
        format!("{}   {}", entry.date(), entry.duration()),
        format!(
            "Result   {}",
            fitted(&entry.result(state.player.as_deref()), 20)
        ),
        entry
            .archive_clip
            .as_ref()
            .map_or_else(|| entry.player_count(), |(_, title)| fitted(title, 28)),
        if film.scoreboard().is_empty() {
            "Scoreboard unavailable".into()
        } else {
            format!(
                "Players {}/{}       Team        Score     K / D",
                state.roster_page + 1,
                roster_pages
            )
        },
    ];
    for i in 0..ROSTER {
        if let Some(p) = film.scoreboard().get(state.roster_page * ROSTER + i) {
            let dash = || "--".to_owned();
            values.extend([
                fitted(&p.name, 15),
                fitted(p.team.as_deref().unwrap_or("--"), 8),
                p.score.map_or_else(dash, |score| score.to_string()),
                format!(
                    "{} / {}",
                    p.kills.map_or_else(dash, |n| n.to_string()),
                    p.deaths.map_or_else(dash, |n| n.to_string())
                ),
            ]);
        } else {
            values.extend(std::iter::repeat_n(String::new(), 4));
        }
    }
    values
}

fn pane_text(i: usize, t: &PanelText, text: String) -> PaneText {
    PaneText {
        id: format!("theater.panel.{i}"),
        rect: t.block.rect,
        font: t.block.font,
        colour: t.block.colour,
        right: t.right,
        text,
    }
}

fn art(p: PictureSpec, size: [u16; 2]) -> PaneArt {
    PaneArt {
        art: Art::Screen {
            screen_id: p.screen_id as i16,
            pane: p.pane as u16,
            index: p.bitmap as u16,
            image: p.image,
        },
        at: p.at,
        size,
        depth: 0,
    }
}

pub(super) fn spec(theater: &Theater, list: bool) -> ScreenSpec {
    let state = theater.state();
    let title = if list {
        "THEATER".to_owned()
    } else {
        state
            .selected
            .as_ref()
            .map_or_else(|| "Film".into(), |e| fitted(&map_title(&e.film.map), 27))
    };
    let mut s = ScreenSpec::new(if list { FILMS } else { DETAIL }, &title);
    s.rows = if list {
        films_model(&state)
    } else {
        detail_model(&state)
    };
    s.footer = true;
    s.style = ValueStyle::Inline;
    s.prompts = prompts(list);
    s.inputs = vec![
        Input::A,
        Input::B,
        Input::Up,
        Input::Down,
        Input::Left,
        Input::Right,
    ];
    s.list = ListPlace::from(Some(if list { FILMS_LAYOUT } else { FILM_LAYOUT }));
    s.description = Some((if list { LIST_STATUS } else { FILM_STATUS }).into());
    if list {
        s.scrolls = true;
        s.texts.push(pane_text(
            0,
            &panel_cell(TAB_STRIP.rect, false, false),
            tabs(&state).join("     "),
        ));
        s.art.extend([
            art(crate::skulls_menu::FRAME, crate::skulls_menu::FRAME_SIZE),
            art(LIST_PICTURE, lobby::PICTURE_SIZE),
        ]);
    } else {
        s.panel = FILM_PANEL_ART
            .into_iter()
            .zip(lobby::ART_SIZES)
            .map(|(p, size)| art(p, size))
            .collect();
        s.art.push(art(FILM_PICTURE, lobby::PICTURE_SIZE));
        s.texts = film_panel()
            .texts
            .iter()
            .zip(film_panel_values(&state))
            .enumerate()
            .map(|(i, (t, value))| pane_text(i, t, value))
            .collect();
        s.help = (!state.status.is_empty()).then(|| fitted(&state.status, 39));
    }
    s
}

struct GuestTexts {
    at: AtomicU32,
    values: Mutex<Vec<Vec<u8>>>,
}

impl Default for GuestTexts {
    fn default() -> Self {
        Self {
            at: AtomicU32::new(0),
            values: Mutex::new(vec![Vec::new(); TEXTS.len as usize]),
        }
    }
}

impl GuestTexts {
    fn set(&self, i: u32, value: &str) -> u32 {
        let mut slots = self.values.lock().unwrap_or_else(|e| e.into_inner());
        let mut end = value.len().min(CAPACITY as usize - 1);
        while !value.is_char_boundary(end) {
            end -= 1;
        }
        let slot = &mut slots[i as usize];
        slot.clear();
        slot.extend(value[..end].bytes().filter(|&b| b != 0));
        slot.push(0);
        TEXTS.id(i)
    }

    fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        let base = self.at.load(Ordering::Relaxed);
        if base != 0 {
            let mut slots = self.values.lock().unwrap_or_else(|e| e.into_inner());
            for (i, bytes) in slots.iter_mut().enumerate() {
                if !bytes.is_empty() {
                    m.write(base + i as u32 * CAPACITY, bytes)?;
                    bytes.clear();
                }
            }
        }
        Ok(())
    }
}

struct ScreenRows {
    theater: Theater,
    texts: Arc<GuestTexts>,
    list: bool,
    drawn: AtomicU64,
    cached: Mutex<Option<(u64, ScreenSpec)>>,
    pictures: Mutex<BTreeMap<String, u32>>,
}

impl ScreenRows {
    fn new(theater: Theater, texts: Arc<GuestTexts>, list: bool) -> Self {
        Self {
            theater,
            texts,
            list,
            drawn: AtomicU64::new(u64::MAX),
            cached: Mutex::new(None),
            pictures: Mutex::new(BTreeMap::new()),
        }
    }

    fn with_spec<T>(&self, get: impl FnOnce(&ScreenSpec) -> T) -> T {
        let revision = self.theater.state().revision;
        let mut cached = self.cached.lock().unwrap_or_else(|e| e.into_inner());
        if cached.as_ref().is_none_or(|(old, _)| *old != revision) {
            *cached = Some((revision, spec(&self.theater, self.list)));
        }
        get(&cached.as_ref().expect("cached Theater spec").1)
    }
}

impl Rows for ScreenRows {
    fn title(&self) -> u32 {
        TEXTS.id(if self.list { FILMS_TITLE } else { DETAIL_TITLE })
    }

    fn count(&self) -> usize {
        self.with_spec(|s| s.rows.len())
    }

    fn growing_capacity(&self) -> Option<usize> {
        self.list.then_some(MAX_ROWS)
    }

    fn shrink_growing_rows(&self) -> bool {
        self.list
    }

    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let index = row as usize;
        let Some((label, _)) = self.with_spec(|s| s.rows.get(index).map(|r| r.cells(s.style)))
        else {
            return Vec::new();
        };
        let slot = if self.list { ROWS } else { DETAIL_ROWS } + row as u32;
        vec![(crate::ui::LABEL_SLOT, self.texts.set(slot, &label))]
    }

    fn press(&self, row: i16) -> Chosen {
        if self.list {
            self.theater.choose_film(row)
        } else {
            self.with_spec(|s| {
                s.rows
                    .get(row as usize)
                    .filter(|r| r.press == Press::Act)
                    .map_or(Chosen::Nothing, |_| Chosen::Act)
            })
        }
    }

    fn sideways(&self, row: i16, right: bool) -> Option<String> {
        let mut state = self.theater.state();
        if self.list {
            let message = match row {
                0 => {
                    state.change_tab(right);
                    let tab = state.tab;
                    drop(state);
                    if tab == Tab::Clips {
                        self.theater.refresh_clips();
                    }
                    return Some(tab.label().into());
                }
                1 => {
                    state.selector.step_sort(right);
                    state.selector.sort.label().into()
                }
                2 | 3 => {
                    let tab = state.tab_entries();
                    let (filter, name) = if row == 2 {
                        (Filter::Map, "Map")
                    } else {
                        (Filter::GameType, "Game type")
                    };
                    state.selector.step_filter(&tab, filter, right);
                    format!("{name} filter changed")
                }
                _ => return None,
            };
            state.changed();
            return Some(message);
        }
        let pages = state
            .selected
            .as_ref()?
            .film
            .scoreboard()
            .len()
            .div_ceil(ROSTER);
        if pages <= 1 {
            return None;
        }
        state.roster_page = (state.roster_page + if right { 1 } else { pages - 1 }) % pages;
        state.changed();
        Some(format!("Players page {}", state.roster_page + 1))
    }

    fn description(&self, row: i16) -> Option<u32> {
        Some(self.with_spec(|s| {
            let message = s
                .rows
                .get(row as usize)
                .and_then(|r| r.description.as_deref());
            let slot = if self.list { STATUS } else { STATUS + 1 };
            self.texts
                .set(slot, s.help.as_deref().or(message).unwrap_or(""))
        }))
    }

    fn panel_text(&self, i: usize) -> Option<u32> {
        // Clear slots from a preceding roster on the same screen.
        let count = if self.list { 1 } else { 8 + ROSTER * 4 };
        let slot = if self.list {
            TAB_PANEL
        } else {
            PANEL + i as u32
        };
        (i < count).then(|| {
            self.with_spec(|s| self.texts.set(slot, s.texts.get(i).map_or("", |t| &t.text)))
        })
    }

    fn picture(&self, row: i16) -> Option<u32> {
        let state = self.theater.state();
        let entry = if self.list {
            entry_at(&state, row)
        } else {
            state.selected.clone()
        }?;
        let pictures = self.pictures.lock().unwrap_or_else(|e| e.into_inner());
        pictures.get(&map_stem(&entry.film.map)).copied()
    }

    fn restore_missing_picture(&self) -> bool {
        true
    }

    fn live_text(&self) -> LiveText {
        let revision = self.theater.state().revision;
        LiveText {
            writes: Vec::new(),
            redraw: self.drawn.swap(revision, Ordering::Relaxed) != revision,
        }
    }

    fn write_texts(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
        if !self.list {
            self.with_spec(|s| self.texts.set(DETAIL_TITLE, &s.title));
        }
        let mut pictures = self.pictures.lock().unwrap_or_else(|e| e.into_inner());
        if pictures.is_empty() {
            *pictures = crate::mod_slots::pictures(&crate::mod_slots::read_slots(
                &crate::mod_slots::WriteGm(m),
            ));
        }
        self.texts.write(m)
    }
}

fn classes(theater: &Theater, texts: &Arc<GuestTexts>) -> [ListClass; 2] {
    let list = Arc::new(ScreenRows::new(theater.clone(), texts.clone(), true));
    let detail = Arc::new(ScreenRows::new(theater.clone(), texts.clone(), false));
    [
        crate::skulls_menu::level_select_list(
            ListClass::with_rows("theater-films", FILMS, list),
            FILMS_LAYOUT,
        )
        .description(LIST_STATUS)
        .picture(LIST_PICTURE)
        .panel(Panel {
            art: Vec::new(),
            texts: vec![panel_cell(TAB_STRIP.rect, false, false)],
        })
        .sideways()
        .footer(TEXTS.id(FILMS_FOOTER)),
        ListClass::with_rows("theater-film", DETAIL, detail)
            .layout(FILM_LAYOUT)
            .description(FILM_STATUS)
            .picture(FILM_PICTURE)
            .panel(film_panel())
            .sideways()
            .action_buttons(&[])
            .action({
                let theater = theater.clone();
                Arc::new(move |cx, screen| {
                    let theater = theater.clone();
                    Box::pin(async move {
                        if let Some(row) = crate::list_class::focused_row(&cx, screen).await?
                            && theater.step_detail(row)
                        {
                            crate::ui_kit::dialog_tag::close(&cx, screen).await?;
                        }
                        Ok(())
                    })
                })
            })
            .on_free({
                let theater = theater.clone();
                Arc::new(move |_, _| {
                    let mut state = theater.state();
                    state.selected = None;
                    state.changed();
                    Box::pin(async { Ok(()) })
                })
            })
            .footer(TEXTS.id(DETAIL_FOOTER)),
    ]
}

struct TextFeature(Arc<GuestTexts>);

impl feature::Feature for TextFeature {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new("theater-texts", "The Theater row and film library texts")
            .doc("docs/game/theater.md")
    }

    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        let at = b.guest_alloc(TEXTS.len * CAPACITY, Permissions::RW)?;
        self.0.at.store(at, Ordering::Relaxed);
        b.locale().source("theater texts", move |id| {
            TEXTS.owns(id).then(|| at + (id - TEXTS.base) * CAPACITY)
        });
        let texts = self.0.clone();
        b.frame().on_tick("theater static texts", move |o| {
            let mut g = o.guest();
            let _ = texts.write(&mut *g);
        });
        b.snapshot(feature::SnapshotPolicy::Stateless);
        Ok(())
    }
}

pub(super) fn install(registry: &mut feature::Registry, theater: Theater) -> anyhow::Result<()> {
    let texts = Arc::new(GuestTexts::default());
    texts.set(FILMS_TITLE, "THEATER");
    texts.set(DETAIL_TITLE, "Film");
    texts.set(FILMS_FOOTER, &legend(&prompts(true)));
    texts.set(DETAIL_FOOTER, &legend(&prompts(false)));
    registry
        .add(&TextFeature(texts.clone()))
        .map_err(anyhow::Error::msg)?;
    for class in classes(&theater, &texts) {
        registry.add(&class).map_err(anyhow::Error::msg)?;
    }
    Ok(())
}

pub(super) fn preview_page(theater: &Theater, list: bool) -> crate::ui_kit::page::ListPage {
    let [films, detail] = classes(theater, &Arc::new(GuestTexts::default()));
    let theater = theater.clone();
    crate::ui_kit::page::ListPage::new(if list { films } else { detail }, move || {
        spec(&theater, list)
    })
}
