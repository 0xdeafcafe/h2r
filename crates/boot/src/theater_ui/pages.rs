use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::{GuestMemory, MemFault};
use runtime::guest::Permissions;

use super::{DETAIL, Entry, FILMS, Load, Theater};
use crate::list_class::{
    ListClass, ListLayout, LiveText, Panel, PanelText, PictureSpec, Rows, TextBlock,
};
use crate::mods_screen::page_layout as lobby;
use crate::ui::Chosen;
use crate::ui_kit::input::{Glyph, Input, Prompt, legend};
use crate::ui_kit::spec::{ListPlace, PaneText, Press, RowSpec, ScreenSpec};
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
pub(super) const MAX_ROWS: usize = 41;
const DETAIL_ROWS: u32 = ROWS + MAX_ROWS as u32 * 2;
const TAB_PANEL: u32 = DETAIL_ROWS + 4;
const PANEL: u32 = TAB_PANEL + 1;

const FILMS_LAYOUT: ListLayout = ListLayout {
    skin: 2,
    anchor: [-254, 113],
    bitmaps_left_of: i16::MIN,
    visible: 10,
};
const FILM_LAYOUT: ListLayout = ListLayout {
    visible: 4,
    ..lobby::LIST
};
const FILM_PICTURE: PictureSpec = PictureSpec {
    at: [-258, 46],
    ..lobby::PICTURE
};
const LIST_STATUS: TextBlock = TextBlock {
    rect: [-147, -245, -169, 245],
    ..lobby::STATUS
};
const FILM_STATUS: TextBlock = TextBlock {
    rect: [66, -258, 47, 40],
    ..lobby::STATUS
};
const TAB_STRIP: TextBlock = TextBlock {
    rect: [170, -244, 146, 244],
    ..lobby::BODY
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Watch,
    Save,
    Delete,
    Web,
}

impl Action {
    const ALL: [Self; 4] = [Self::Watch, Self::Save, Self::Delete, Self::Web];
}

impl Theater {
    fn choose_film(&self, row: i16) -> Chosen {
        let mut state = self.state();
        if row == 0 && matches!(state.load, Load::Error(_)) {
            let player = state.player.clone();
            drop(state);
            self.open(player);
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
        let Some(entry) = state.selected.clone() else {
            return false;
        };
        let Some(&action) = usize::try_from(row).ok().and_then(|i| Action::ALL.get(i)) else {
            return false;
        };
        if action != Action::Delete {
            state.confirm_delete = false;
        }
        state.status.clear();
        match action {
            Action::Watch if entry.film.playable_here => {
                drop(state);
                let answer = self.session().play(&entry);
                if let Err(error) = answer {
                    let mut state = self.state();
                    state.status = error;
                    state.changed();
                }
                return false;
            }
            Action::Watch => {}
            Action::Web if entry.film.source == super::adapter::Source::Host => {
                state.web = !state.web;
            }
            Action::Web => {}
            Action::Delete if !state.confirm_delete => {
                state.confirm_delete = true;
                state.status = "Press A again to delete".into();
            }
            Action::Save | Action::Delete => {
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
    let index = usize::try_from(row.checked_sub(1)?).ok()?;
    state.entries().into_iter().nth(index)
}

fn clock(ticks: u64) -> String {
    let s = ticks / 30;
    format!("{}:{:02}", s / 60, s % 60)
}

fn fitted(s: &str, limit: usize) -> String {
    let clean = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= limit {
        clean
    } else {
        format!(
            "{}...",
            clean
                .chars()
                .take(limit.saturating_sub(3))
                .collect::<String>()
        )
    }
}

fn film_line(entry: &Entry, player: Option<&str>) -> (String, String) {
    let film = &entry.film;
    let map = content::map_pack::title(&film.map).unwrap_or(&film.map);
    let mark = if film.playable_here { "" } else { "* " };
    let label = format!("{mark}{} / {}", fitted(map, 13), fitted(&film.variant, 13));
    let date = i64::try_from(film.started_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(
            || "Unknown date".into(),
            |date| date.format("%d %b %y").to_string(),
        );
    let duration = clock(
        entry
            .clip
            .as_ref()
            .map_or(film.length_ticks, |c| c.to_tick - c.from_tick),
    );
    let own_score = player.and_then(|name| {
        film.players
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))?
            .score
    });
    let result = if let Some(result) = &film.result {
        fitted(result, 9)
    } else if let Some(score) = own_score {
        format!("{score} pts")
    } else {
        "--".into()
    };
    (label, format!("{date}   {duration}   {result}"))
}

fn tabs(state: &super::State) -> Vec<String> {
    super::Tab::ALL
        .into_iter()
        .map(|tab| {
            if state.tab == tab {
                format!("[ {} ]", tab.label())
            } else {
                tab.label().to_owned()
            }
        })
        .collect()
}

fn films_model(state: &super::State) -> Vec<RowSpec> {
    let mut rows = Vec::new();
    let (label, value, press) = match &state.load {
        Load::Loading => ("Reading films...", "", Press::Nothing),
        Load::Error(_) => ("Retry library", "", Press::Act),
        Load::Ready(_) if state.entries().is_empty() => (
            match state.tab {
                super::Tab::Recent => "No recent films",
                super::Tab::Saved => "No saved films",
                super::Tab::Clips => "No clips",
            },
            "",
            Press::Nothing,
        ),
        Load::Ready(_) => ("Map / Game type", "Date / Length / Result", Press::Nothing),
    };
    rows.push(RowSpec::info(label, value).press(press));
    rows.extend(state.entries().iter().take(MAX_ROWS - 1).map(|entry| {
        let (label, meta) = film_line(entry, state.player.as_deref());
        let row = RowSpec::info(&label, &meta).press(Press::Open(DETAIL.into()));
        if entry.film.playable_here {
            row
        } else {
            row.describe("Unavailable on this build")
        }
    }));
    rows
}

fn detail_model(state: &super::State) -> Vec<RowSpec> {
    let Some(entry) = &state.selected else {
        return vec![RowSpec::info("Film unavailable", "")];
    };
    let film = &entry.film;
    Action::ALL
        .into_iter()
        .map(|action| {
            let (label, enabled) = match action {
                Action::Watch if !film.playable_here => ("Watch (build mismatch)", false),
                Action::Watch => ("Watch", true),
                Action::Save if film.saved => ("Unsave", true),
                Action::Save => ("Save", true),
                Action::Delete if state.confirm_delete => ("Confirm delete", true),
                Action::Delete => ("Delete", true),
                Action::Web if state.web => ("Hide web link", true),
                Action::Web => (
                    "Open on the web",
                    film.source == super::adapter::Source::Host,
                ),
            };
            if enabled {
                RowSpec::action(label).press(Press::Act)
            } else {
                RowSpec::info(label, "")
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
        if list { "Tabs (top row)" } else { "Players" },
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
    for i in 0..4 {
        let y = 159 - i * 19;
        texts.push(panel_cell([y, 52, y - 18, 272], false, true));
    }
    texts.push(panel_cell([80, -12, 61, 272], false, false));
    for row in 0..ROSTER as i16 {
        let y = 60 - row * 20;
        for (left, right, numeric) in [
            (-12, 110, false),
            (113, 172, false),
            (174, 211, true),
            (214, 272, true),
        ] {
            texts.push(panel_cell([y, left, y - 19, right], numeric, true));
        }
    }
    texts.push(panel_cell([-164, -258, -184, 272], false, true));
    Panel {
        art: lobby::ART.to_vec(),
        texts,
    }
}

fn film_panel_values(state: &super::State, web: Option<String>) -> Vec<String> {
    let Some(entry) = &state.selected else {
        return Vec::new();
    };
    let f = &entry.film;
    let host = match f.source {
        super::adapter::Source::Host => "Dedicated host",
        super::adapter::Source::Local => "This console",
    };
    let length = entry
        .clip
        .as_ref()
        .map_or(f.length_ticks, |c| c.to_tick - c.from_tick);
    let stamp = f.image_stamp.trim().lines().next().unwrap_or_default();
    let roster_pages = f.players.len().div_ceil(ROSTER).max(1);
    let mut values = vec![
        "Film details".into(),
        format!("Variant   {}", fitted(&f.variant, 20)),
        format!("Length   {}", clock(length)),
        format!("Host   {}", fitted(f.host.as_deref().unwrap_or(host), 20)),
        format!("Build   {}", fitted(stamp, 21)),
        format!(
            "Players {}/{}       Team        Score     K / D",
            state.roster_page + 1,
            roster_pages
        ),
    ];
    for i in 0..ROSTER {
        if let Some(p) = f.players.get(state.roster_page * ROSTER + i) {
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
    values.push(web.unwrap_or_default());
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
    let link = theater.web_url();
    let state = theater.state();
    let title = if list {
        "Films".to_owned()
    } else {
        state.selected.as_ref().map_or_else(
            || "Film".into(),
            |e| {
                fitted(
                    content::map_pack::title(&e.film.map).unwrap_or(&e.film.map),
                    27,
                )
            },
        )
    };
    let mut s = ScreenSpec::new(if list { FILMS } else { DETAIL }, &title);
    s.rows = if list {
        films_model(&state)
    } else {
        detail_model(&state)
    };
    s.footer = true;
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
        s.help = match &state.load {
            Load::Error(error) => Some(fitted(error, 38)),
            Load::Ready(_) if state.tab == super::Tab::Clips => state.clips_error.clone(),
            _ => None,
        };
    } else {
        s.style = crate::ui_kit::spec::ValueStyle::Inline;
        s.panel = lobby::ART
            .into_iter()
            .zip(lobby::ART_SIZES)
            .map(|(p, size)| art(p, size))
            .collect();
        s.art.push(art(FILM_PICTURE, lobby::PICTURE_SIZE));
        s.texts = film_panel()
            .texts
            .iter()
            .zip(film_panel_values(&state, link))
            .enumerate()
            .map(|(i, (t, value))| pane_text(i, t, value))
            .collect();
        s.help = if !state.status.is_empty() {
            Some(fitted(&state.status, 39))
        } else if state
            .selected
            .as_ref()
            .is_some_and(|entry| !entry.film.playable_here)
        {
            Some("Unavailable on this build".into())
        } else {
            Some(String::new())
        };
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
        let Some((label, value)) = self.with_spec(|s| s.rows.get(index).map(|r| r.cells(s.style)))
        else {
            return Vec::new();
        };
        let slot = if self.list {
            ROWS + row as u32 * 2
        } else {
            DETAIL_ROWS + row as u32
        };
        let id = self.texts.set(slot, &label);
        let mut cells = vec![(crate::ui::LABEL_SLOT, id)];
        if self.list {
            cells.push((
                crate::ui::VALUE_SLOT,
                self.texts
                    .set(ROWS + row as u32 * 2 + 1, value.as_deref().unwrap_or("")),
            ));
        }
        cells
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
            if row != 0 {
                return None;
            }
            state.change_tab(right);
            let tab = state.tab;
            drop(state);
            if tab == super::Tab::Clips {
                self.theater.refresh_clips();
            }
            return Some(tab.label().to_owned());
        }
        let pages = state.selected.as_ref()?.film.players.len().div_ceil(ROSTER);
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
                .set(slot, message.or(s.help.as_deref()).unwrap_or(""))
        }))
    }

    fn panel_text(&self, i: usize) -> Option<u32> {
        // Clear slots from a preceding roster or web view on the same screen.
        let count = if self.list { 1 } else { 7 + ROSTER * 4 };
        let slot = if self.list {
            TAB_PANEL
        } else {
            PANEL + i as u32
        };
        (i < count).then(|| {
            self.with_spec(|s| self.texts.set(slot, s.texts.get(i).map_or("", |t| &t.text)))
        })
    }

    fn picture(&self, _row: i16) -> Option<u32> {
        let state = self.theater.state();
        let map = &state.selected.as_ref()?.film.map;
        let pictures = self.pictures.lock().unwrap_or_else(|e| e.into_inner());
        let stem = map.rsplit(['/', '\\']).next().unwrap_or(map);
        let stem = stem.strip_suffix(".map").unwrap_or(stem);
        pictures.get(&stem.to_ascii_lowercase()).copied()
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
            let mut pictures = self.pictures.lock().unwrap_or_else(|e| e.into_inner());
            if pictures.is_empty() {
                *pictures = crate::mod_slots::pictures(&crate::mod_slots::read_slots(
                    &crate::mod_slots::WriteGm(m),
                ));
            }
        }
        self.texts.write(m)
    }
}

fn classes(theater: &Theater, texts: &Arc<GuestTexts>) -> [ListClass; 2] {
    let list = Arc::new(ScreenRows::new(theater.clone(), texts.clone(), true));
    let detail = Arc::new(ScreenRows::new(theater.clone(), texts.clone(), false));
    [
        ListClass::with_rows("theater-films", FILMS, list)
            .layout(FILMS_LAYOUT)
            .description(LIST_STATUS)
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
                    state.web = false;
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
    texts.set(FILMS_TITLE, "Films");
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
