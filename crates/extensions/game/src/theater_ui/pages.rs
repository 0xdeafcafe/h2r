use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use guest_layout::memory::{GuestMemory, MemFault};
use runtime::guest::Permissions;

use super::selector::{OptionRow, map_title};
use super::{DETAIL, Entry, FILMS, Load, Tab, Theater, adapter::FilmSummary};
use crate::list_class::{
    ListClass, ListLayout, LiveText, Panel, PanelText, PictureSpec, Rows, TextBlock,
};
use crate::mods_screen::page_layout as lobby;
use crate::ui::Chosen;
use crate::ui_kit::input::{Glyph, Input, Prompt, footer_legend};
use crate::ui_kit::level_art;
use crate::ui_kit::spec::{ListPlace, PaneText, Press, RowSpec, ScreenSpec, ValueStyle};
use crate::ui_kit::text_fit;
use crate::ui_kit::view::{Art, PaneArt};

pub(super) const TEXTS: crate::ui_kit::ids::Family = crate::ui_kit::ids::family("theater");
const CAPACITY: u32 = 512;
const FILMS_TITLE: u32 = 1;
const DETAIL_TITLE: u32 = 2;
const FILMS_FOOTER: u32 = 3;
const DETAIL_FOOTER: u32 = 4;
const STATUS: u32 = 5;
const ROWS: u32 = 8;
const ROSTER: usize = 10;
pub(super) const FILM_ROWS: usize = 40;
pub(super) const BROWSE_ROWS: usize = OptionRow::ALL.len();
const MAX_ROWS: usize = FILM_ROWS + BROWSE_ROWS + 1; // Also allow a retry row.
const DETAIL_ROWS: u32 = ROWS + MAX_ROWS as u32;
const DETAIL_ACTIONS: i16 = 6;
const TAB_PANEL: u32 = DETAIL_ROWS + DETAIL_ACTIONS as u32;
const TAB_TEXTS: usize = 2 * Tab::ALL.len();
const PANEL: u32 = TAB_PANEL + TAB_TEXTS as u32 + 2;
const _: () = assert!(PANEL + crate::list_class::MAX_PANEL_TEXTS as u32 <= TEXTS.len);
/// The first slot the library and the film page leave free: the pause
/// menu's start no lower (`pause_screen`).
pub(super) const SPARE: u32 = PANEL + crate::list_class::MAX_PANEL_TEXTS as u32;

// The single-column skin leaves the right half for the selected film.
const FILMS_LAYOUT: ListLayout = ListLayout {
    anchor: [-262, 110],
    visible: 13,
    ..lobby::LIST
};
const LIST_PICTURE: PictureSpec = PictureSpec {
    at: [22, 149],
    ..lobby::PICTURE
};
// The actions list shows all six rows; the fact block sits under it.
const FILM_VISIBLE: i16 = 6;
const FILM_LAYOUT: ListLayout = ListLayout {
    anchor: lobby::LIST.anchor,
    visible: FILM_VISIBLE,
    ..lobby::LIST
};
const FILM_PICTURE: PictureSpec = PictureSpec {
    at: [12, 174],
    ..lobby::PICTURE
};
// The details and the empty library's help start at the map picture's
// left edge and end at the panel's right.
const DETAIL_LEFT: i16 = LIST_PICTURE.at[0];
const DETAIL_RIGHT: i16 = 268;
const LIST_STATUS: TextBlock = TextBlock {
    rect: [-62, DETAIL_LEFT, -184, DETAIL_RIGHT],
    ..lobby::BODY
};
const FILM_STATUS: TextBlock = TextBlock {
    rect: [-148, -258, -187, -42],
    ..lobby::STATUS
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Watch,
    Save,
    Delete,
    Share,
    CopyLink,
}

impl Action {
    fn for_entry(entry: &Entry) -> impl Iterator<Item = Self> {
        [
            Self::Watch,
            Self::Save,
            Self::Delete,
            Self::Share,
            Self::CopyLink,
        ]
        .into_iter()
        .filter(move |action| match action {
            Self::Watch => entry.film.watch_unavailable().is_none(),
            Self::Save => entry.clip.is_none(),
            Self::Share | Self::CopyLink => {
                entry.film.source != super::adapter::Source::Archive
                    && (entry.film.version == Some(4) || entry.film.live_game_id.is_some()
                        || entry.film.archive_id().is_some())
            }
            Self::Delete => {
                entry.clip.is_some() || entry.film.source != super::adapter::Source::Archive
            }
        })
    }

    pub(super) fn at(entry: &Entry, row: usize) -> Option<Self> {
        Self::for_entry(entry).nth(row)
    }

    fn blocked(self, state: &super::State) -> bool {
        state.working
    }
}

impl Theater {
    fn choose_film(&self, row: i16) -> Chosen {
        let mut state = self.state();
        if row == BROWSE_ROWS as i16 && state.library_error().is_some() {
            let (controller, player) = (state.controller, state.player.clone());
            drop(state);
            self.open(controller, player);
            return Chosen::Stepped("Refreshing films".into());
        }
        if let Some(option) = OptionRow::at(row) {
            let pick = option.pick(self, &state);
            return crate::ui_kit::picker::offer_list(pick).map_or(Chosen::Nothing, Chosen::Open);
        }
        let Some(entry) = entry_at(&state, row) else {
            return Chosen::Nothing;
        };
        state.select(entry);
        Chosen::Push(DETAIL)
    }

    pub fn step_detail(&self, row: i16) -> bool {
        let mut state = self.state();
        let Some(entry) = state.selected.clone() else {
            return false;
        };
        let Some(action) = usize::try_from(row)
            .ok()
            .and_then(|i| Action::at(&entry, i))
        else {
            return false;
        };
        if action.blocked(&state) {
            return false;
        }
        if action != Action::Delete {
            state.confirm_delete = false;
        }
        state.status.clear();
        match action {
            Action::Watch => {
                if entry.film.archive_id().is_some() {
                    drop(state);
                    self.archive_action(Action::Watch, entry);
                    return false;
                }
                let view = state.view;
                state.status = "Opening film...".into();
                state.changed();
                drop(state);
                if let Err(error) = self.play(&entry, view) {
                    let mut state = self.state();
                    state.status = error;
                    state.changed();
                }
                return false;
            }
            Action::Share | Action::CopyLink => {
                drop(state);
                self.archive_action(action, entry);
                return false;
            }
            Action::Delete if !state.confirm_delete => {
                state.confirm_delete = true;
                state.status = "Press A again to delete".into();
            }
            Action::Save | Action::Delete => {
                if (entry.film.live_game_id.is_some() && entry.clip.is_none())
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
    let controls = BROWSE_ROWS + usize::from(state.library_error().is_some());
    let index = usize::try_from(row).ok()?.checked_sub(controls)?;
    state.entries().into_iter().nth(index)
}

// The film's detail page frames its right-hand level picture.
fn film_art() -> [(PictureSpec, [u16; 2]); 2] {
    use crate::ui_kit::dialog::{FRAME_CORNER_SIZE, FRAME_CORNERS, LARGE_SCREEN_ID};

    let picture = FILM_PICTURE;
    let [left, top] = picture.at;
    let [width, height] = lobby::PICTURE_SIZE.map(|n| n as i16);
    let side = FRAME_CORNER_SIZE as i16;
    let corner = |index: u16, at| {
        (
            PictureSpec {
                screen_id: LARGE_SCREEN_ID as u32,
                pane: 0,
                bitmap: u32::from(index),
                image: 0,
                at,
                animation: picture.animation,
            },
            [FRAME_CORNER_SIZE; 2],
        )
    };
    [
        corner(FRAME_CORNERS[0], [left, top]),
        corner(FRAME_CORNERS[1], [left + width - side, top - height + side]),
    ]
}

fn library_panel() -> Panel {
    // Each tab has a bright and a muted text at the same position. The
    // inactive text of each pair is hidden.
    let mut texts: Vec<_> = (0..Tab::ALL.len())
        .flat_map(|i| {
            let left = -242 + 80 * i as i16;
            let rect = [174, left, 152, left + 74];
            [panel_cell(rect, false, false), panel_cell(rect, false, true)]
        })
        .collect();
    texts.push(panel_cell([174, 22, 152, 268], true, true));
    texts.push(panel_cell([140, DETAIL_LEFT, 20, DETAIL_RIGHT], false, true));
    Panel {
        art: Vec::new(),
        texts,
    }
}

fn library_count(state: &super::State, count: usize) -> String {
    let noun = if state.tab == Tab::Clips { "clip" } else { "film" };
    format!("{count} {noun}{}", if count == 1 { "" } else { "s" })
}

fn empty_help(state: &super::State) -> String {
    if let Some(error) = state.library_error().or_else(|| {
        state
            .clips_error
            .as_deref()
            .filter(|_| state.tab == Tab::Clips)
    }) {
        return text_fit::paragraph(error, super::selector::DETAIL_WIDTH, 5);
    }
    let message = match state.load {
        Load::Loading => return "Reading films...".into(),
        Load::Ready(_) if state.selector.map.is_some() || state.selector.game_type.is_some() => {
            ["No matching films", "Choose All in Map or Game type."]
        }
        Load::Ready(_) => match state.tab {
            Tab::Recent => ["No recent films", "Play a game to record a film."],
            Tab::Saved => ["No saved films", "Open a film in Recent and choose Save."],
            Tab::Clips => [
                "No clips",
                "While watching a film, press X to mark the start and end of a clip.",
            ],
        },
    };
    crate::ui_kit::text::join_sentences(message)
}

fn films_model(state: &super::State, entries: &[Entry]) -> Vec<RowSpec> {
    let mut rows = Vec::with_capacity(BROWSE_ROWS + entries.len() + 1);
    rows.extend(OptionRow::ALL.map(|option| {
        let label = option.label();
        let room = text_fit::ROW - text_fit::width(label) - text_fit::width(": ");
        RowSpec::info(label, &text_fit::line(option.current(&state.selector), room))
            .press(Press::Open(crate::ui_kit::picker::NAME.into()))
            .describe(option.description())
    }));
    if let Some(error) = state.library_error() {
        let mut retry = RowSpec::action("Retry library")
            .describe(&text_fit::paragraph(error, super::selector::DETAIL_WIDTH, 5));
        retry.prompts = Some(library_prompts("Retry"));
        rows.push(retry);
    }
    rows.extend(entries.iter().map(|entry| {
        let when = entry.recorded_time();
        let mut details = entry.browse_description(state.player.as_deref());
        if !when.is_empty() {
            details.insert_str(0, &format!("{when}\n"));
        }
        let mut row = RowSpec::action(&entry.row_title())
            .press(Press::Open(DETAIL.into()))
            .describe(&details);
        row.prompts = Some(library_prompts("Details"));
        row.art = level_picture(entry, LIST_PICTURE);
        row
    }));
    rows
}

fn detail_model(state: &super::State) -> Vec<RowSpec> {
    let Some(entry) = &state.selected else {
        return vec![RowSpec::info("Film unavailable", "")];
    };
    let film = &entry.film;
    Action::for_entry(entry)
        .map(|action| {
            let label: &str = match action {
                Action::Watch => "Watch",
                Action::Share => "Share",
                Action::CopyLink => "Copy Link",
                Action::Save if film.saved => "Unsave",
                Action::Save => "Save",
                Action::Delete if state.confirm_delete => "Confirm delete",
                Action::Delete => "Delete",
            };
            if action.blocked(state) {
                RowSpec::info(label, "")
            } else {
                RowSpec::action(label)
            }
        })
        .collect()
}

fn prompts(action: &str, roster_pages: bool) -> Vec<Prompt> {
    let mut prompts = vec![
        Prompt::new(&[Glyph::A], action),
        Prompt::new(&[Glyph::B], "Back"),
    ];
    if roster_pages {
        prompts.push(Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Players"));
    }
    prompts
}

fn library_prompts(action: &str) -> Vec<Prompt> {
    let mut footer = prompts(action, false);
    footer.push(Prompt::new(&[Glyph::DpadLeft, Glyph::DpadRight], "Library"));
    footer
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

// The actions and facts share the left column; the map picture and the
// player list share the right.
const FACTS: usize = 6;
const FACT_TOP: i16 = 30;
const CELL_ROWS: usize = ROSTER / 2;
const PLAYERS_TOP: i16 = -41;
const CELLS_TOP: i16 = -63;
// Left and right cell: (name left, name right, value left, value right).
const CELLS: [(i16, i16, i16, i16); 2] = [(-28, 63, 67, 119), (125, 216, 220, 272)];
const PLAYERS_LEFT: i16 = -28;
const PLAYERS_RIGHT: i16 = 272;
const HEADER_TEXT: usize = 2 * FACTS;
const PAGE_TEXT: usize = HEADER_TEXT + 1;
const CELL_TEXT: usize = PAGE_TEXT + 1;
const NO_PLAYERS_TEXT: usize = CELL_TEXT + 2 * ROSTER;
const _: () = assert!(NO_PLAYERS_TEXT < crate::list_class::MAX_PANEL_TEXTS);

fn film_panel() -> Panel {
    let mut texts = Vec::new();
    for k in 0..FACTS {
        let top = FACT_TOP - 22 * k as i16;
        texts.push(panel_cell([top, -258, top - 20, -196], false, false));
        texts.push(panel_cell([top, -192, top - 20, -42], false, true));
    }
    let head = |left, right| [PLAYERS_TOP, left, PLAYERS_TOP - 20, right];
    texts.push(panel_cell(head(PLAYERS_LEFT, 119), false, false));
    texts.push(panel_cell(head(125, PLAYERS_RIGHT), true, true));
    for col in 0..2 {
        let (nl, nr, vl, vr) = CELLS[col];
        for r in 0..CELL_ROWS {
            let top = CELLS_TOP - 22 * r as i16;
            texts.push(panel_cell([top, nl, top - 20, nr], false, false));
            texts.push(panel_cell([top, vl, top - 20, vr], true, true));
        }
    }
    // Shares the header's line; only one of the two ever has text.
    texts.push(panel_cell(head(PLAYERS_LEFT, PLAYERS_RIGHT), false, true));
    Panel {
        art: film_art().map(|(p, _)| p).to_vec(),
        texts,
    }
}

/// Label and value pairs for the fact block: empty values are skipped and
/// the rest pack upward.
fn facts(entry: &Entry, player: Option<&str>) -> Vec<(&'static str, String)> {
    let film = &entry.film;
    let clip = entry
        .archive_clip
        .as_ref()
        .map(|(_, title)| title.trim().to_owned())
        .or_else(|| entry.clip.as_ref().map(|c| c.title.trim().to_owned()))
        .unwrap_or_default();
    let variant = film.variant_name().trim();
    let game = if variant.is_empty() {
        film.game_type().unwrap_or("").trim()
    } else {
        variant
    };
    [
        ("Clip", clip),
        ("Map", map_title(&film.map)),
        ("Game", game.to_owned()),
        ("Mode", film.mode().unwrap_or("").trim().to_owned()),
        ("Date", entry.recorded_date()),
        ("Length", entry.duration().unwrap_or_default()),
        ("Result", entry.result(player).unwrap_or_default()),
    ]
    .into_iter()
    .filter(|(_, value)| !value.trim().is_empty())
    .take(FACTS)
    .collect()
}

/// The listed players: team games sort by team name, keeping order within a team.
fn listed_players(entry: &Entry) -> Vec<&super::adapter::Player> {
    let mut players: Vec<_> = entry.film.players().iter().collect();
    if players.iter().any(|p| p.team.is_some()) {
        players.sort_by_key(|p| (p.team.is_none(), p.team.clone()));
    }
    players
}

fn film_panel_values(state: &super::State) -> Vec<String> {
    let Some(entry) = &state.selected else {
        return Vec::new();
    };
    let mut values = Vec::new();
    let facts = facts(entry, state.player.as_deref());
    for k in 0..FACTS {
        match facts.get(k) {
            Some((label, value)) => values.extend([(*label).to_owned(), value.clone()]),
            None => values.extend([String::new(), String::new()]),
        }
    }
    let players = listed_players(entry);
    let pages = players.len().div_ceil(ROSTER);
    let page = state.roster_page.min(pages.saturating_sub(1));
    values.push(if players.is_empty() { "" } else { "Players" }.into());
    values.push(if pages > 1 {
        format!("{} / {pages}", page + 1)
    } else {
        String::new()
    });
    let teams = players.iter().any(|p| p.team.is_some());
    let scores = players.iter().any(|p| p.score.is_some());
    for col in 0..2 {
        for r in 0..CELL_ROWS {
            match players.get(page * ROSTER + col * CELL_ROWS + r) {
                Some(p) => {
                    values.push(p.name.clone());
                    values.push(if teams {
                        p.team.clone().unwrap_or_default()
                    } else if scores {
                        p.score.map_or_else(String::new, |n| n.to_string())
                    } else {
                        String::new()
                    });
                }
                None => values.extend([String::new(), String::new()]),
            }
        }
    }
    values.push(
        if players.is_empty() {
            "Player list unavailable."
        } else {
            ""
        }
        .into(),
    );
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

fn level_picture(entry: &Entry, picture: PictureSpec) -> Option<PaneArt> {
    let stem = level_art::stem(&entry.film.map);
    (!stem.is_empty()).then_some(PaneArt {
        art: Art::Level { stem },
        at: picture.at,
        size: lobby::PICTURE_SIZE,
        depth: 0,
    })
}

pub(super) fn spec(theater: &Theater, list: bool) -> ScreenSpec {
    let state = theater.state();
    let title = if list {
        "THEATER".to_owned()
    } else {
        state.selected.as_ref().map_or_else(
            || "Film".into(),
            |e| text_fit::line(&map_title(&e.film.map), text_fit::TITLE),
        )
    };
    let mut s = ScreenSpec::new(if list { FILMS } else { DETAIL }, &title);
    let entries = if list { state.entries() } else { Vec::new() };
    s.rows = if list {
        films_model(&state, &entries)
    } else {
        detail_model(&state)
    };
    s.footer = true;
    s.style = ValueStyle::Inline;
    let roster_pages = !list
        && state
            .selected
            .as_ref()
            .is_some_and(|e| e.film.players().len() > ROSTER);
    s.prompts = if list {
        library_prompts("Select")
    } else {
        let mut legend = prompts("Select", roster_pages);
        if s.rows.iter().all(|row| row.press == Press::Nothing) {
            legend.retain(|p| !p.glyphs.contains(&Glyph::A));
        }
        // A blocked action (any action while one runs) takes no A, so its own
        // legend leaves Select out.
        let mut blocked = legend.clone();
        blocked.retain(|p| !p.glyphs.contains(&Glyph::A));
        for row in s.rows.iter_mut().filter(|row| row.press == Press::Nothing) {
            row.prompts = Some(blocked.clone());
        }
        legend
    };
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
        s.content.row_help = true;
        if entries.is_empty() {
            s.content.empty_text = Some(format!("theater.panel.{}", TAB_TEXTS + 1));
        }
        let panel = library_panel();
        s.texts = panel
            .texts
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let text = if i < TAB_TEXTS {
                    let tab = Tab::ALL[i / 2];
                    match (i % 2, state.tab == tab) {
                        (0, true) => tab.label().to_uppercase(),
                        (1, false) => tab.label().into(),
                        _ => String::new(),
                    }
                } else if i == TAB_TEXTS {
                    library_count(&state, entries.len())
                } else if entries.is_empty() {
                    empty_help(&state)
                } else {
                    String::new()
                };
                pane_text(i, cell, text)
            })
            .collect();
    } else {
        s.scrolls = s.rows.len() > FILM_LAYOUT.visible as usize;
        s.panel = film_art().into_iter().map(|(p, size)| art(p, size)).collect();
        s.art.extend(
            state
                .selected
                .as_ref()
                .and_then(|entry| level_picture(entry, FILM_PICTURE)),
        );
        s.texts = film_panel()
            .texts
            .iter()
            .zip(film_panel_values(&state))
            .enumerate()
            .map(|(i, (t, value))| {
                let [_, left, _, right] = t.block.rect;
                pane_text(i, t, text_fit::line(&value, i32::from(right - left)))
            })
            .collect();
        let message = if !state.status.is_empty() {
            Some(state.status.as_str())
        } else {
            state
                .selected
                .as_ref()
                .and_then(|e| e.film.watch_unavailable())
        };
        s.help = message.map(|message| {
            text_fit::paragraph(
                message,
                i32::from(FILM_STATUS.rect[3] - FILM_STATUS.rect[1]),
                2,
            )
        });
    }
    s
}

pub(super) struct GuestTexts {
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
    /// The game's string scanner ends a line at `\r` and ignores `\n`, so a
    /// wrapped paragraph's `\n` would join its lines without the space the
    /// break replaced ("its\nsimulation" reads "itssimulation"); ours break
    /// with `\r\n`, as the game's own strings do.
    pub(super) fn set(&self, i: u32, value: &str) -> u32 {
        let value = value.replace("\r\n", "\n").replace('\n', "\r\n");
        let value = value.as_str();
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

    pub(super) fn write(&self, m: &mut dyn GuestMemory) -> Result<(), MemFault> {
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
    fn content_spec(&self) -> Option<crate::ui_kit::spec::ScreenSpec> {
        Some(self.with_spec(Clone::clone))
    }

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

    /// The library stops at its ends: Up on Sort and Down on the last film
    /// stay put.
    fn wraps(&self) -> bool {
        !self.list
    }

    fn texts(&self, row: i16) -> Vec<(u32, u32)> {
        let index = row as usize;
        let Some(label) = self.with_spec(|s| s.rows.get(index).map(|r| r.cells(s.style).0)) else {
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

    fn sideways(&self, _row: i16, right: bool) -> Option<String> {
        let mut state = self.theater.state();
        if self.list {
            state.change_tab(right);
            let tab = state.tab;
            drop(state);
            if tab == Tab::Clips {
                self.theater.refresh_clips();
            }
            return Some(tab.label().into());
        }
        let pages = state
            .selected
            .as_ref()?
            .film
            .players()
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
        self.with_spec(|s| {
            let message = s
                .rows
                .get(row as usize)
                .and_then(|r| r.description.as_deref());
            let slot = if self.list { STATUS } else { STATUS + 1 };
            s.help
                .as_deref()
                .or(message)
                .filter(|s| !s.is_empty())
                .map(|s| self.texts.set(slot, s))
        })
    }

    fn hide_empty_pane_texts(&self) -> bool {
        true
    }

    fn focused_footer(&self, row: Option<i16>) -> Option<u32> {
        let slot = if self.list {
            FILMS_FOOTER
        } else {
            DETAIL_FOOTER
        };
        Some(self.with_spec(|s| {
            let prompts = row
                .and_then(|r| s.rows.get(r as usize))
                .and_then(|r| r.prompts.as_deref())
                .unwrap_or(&s.prompts);
            self.texts.set(slot, &footer_legend(prompts))
        }))
    }

    fn panel_text(&self, i: usize) -> Option<u32> {
        let slot = if self.list { TAB_PANEL } else { PANEL } + i as u32;
        self.with_spec(|s| {
            s.texts
                .get(i)
                .filter(|t| !t.text.is_empty())
                .map(|t| self.texts.set(slot, &t.text))
        })
    }

    fn picture(&self, row: i16) -> Option<u32> {
        self.with_spec(|spec| {
            let picture = if self.list {
                spec.rows.get(usize::try_from(row).ok()?)?.art.as_ref()?
            } else {
                spec.art.first()?
            };
            let Art::Level { stem } = &picture.art else {
                return None;
            };
            self.pictures
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(stem)
                .copied()
        })
    }

    fn prepare_picture(&self, m: &dyn GuestMemory, template: u32) -> Option<u32> {
        *self.pictures.lock().unwrap_or_else(|e| e.into_inner()) = level_art::loaded(m, template);
        // The lobby picture block names unknown_map. Its null reference
        // keeps the frame clear while no focused film has its own bitmap.
        Some(u32::MAX)
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
            .picture(LIST_PICTURE)
            .panel(library_panel())
            .sideways()
            .footer(TEXTS.id(FILMS_FOOTER)),
        // Not `theater-film`: that is the film observer's id
        // (theater::OBSERVER_FEATURE), planned with these screens whenever
        // a game records.
        ListClass::with_rows("theater-film-detail", DETAIL, detail)
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

impl hooks::Feature for TextFeature {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("theater-texts", "The Theater row and film library texts")
            .doc("docs/game/theater.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
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
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

pub(super) fn install(registry: &mut hooks::Registry, theater: Theater) -> anyhow::Result<()> {
    registry
        .add(&super::link::Driver(theater.clone()))
        .map_err(anyhow::Error::msg)?;
    let texts = Arc::new(GuestTexts::default());
    texts.set(FILMS_TITLE, "THEATER");
    texts.set(DETAIL_TITLE, "Film");
    texts.set(FILMS_FOOTER, &footer_legend(&library_prompts("Select")));
    texts.set(DETAIL_FOOTER, &footer_legend(&prompts("Select", false)));
    registry
        .add(&TextFeature(texts.clone()))
        .map_err(anyhow::Error::msg)?;
    for class in classes(&theater, &texts) {
        registry.add(&class).map_err(anyhow::Error::msg)?;
    }
    super::pause_screen::install(registry, &theater, &texts)
}

pub(super) fn preview_page(theater: &Theater, list: bool) -> crate::ui_kit::page::ListPage {
    let [films, detail] = classes(theater, &Arc::new(GuestTexts::default()));
    let theater = theater.clone();
    crate::ui_kit::page::ListPage::new(if list { films } else { detail }, move || {
        spec(&theater, list)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theater_ui::adapter::FilmMeta;

    /// Every game records by default, so the film observer is planned with
    /// these screens: a screen with its id fails the plan ("two features
    /// have the id theater-film").
    #[test]
    fn no_screen_takes_the_film_observers_id() {
        let theater = Theater::default();
        for class in classes(&theater, &Arc::new(GuestTexts::default())) {
            assert_ne!(class.id, crate::theater::OBSERVER_FEATURE);
        }
    }

    #[test]
    fn guest_rows_write_each_label_and_footer_to_its_own_slot() {
        let theater = Theater::default();
        theater.state().load = Load::Ready(vec![FilmMeta {
            id: "sample".into(),
            map: "lockout".into(),
            ..FilmMeta::default()
        }]);
        let texts = Arc::new(GuestTexts::default());
        let rows = ScreenRows::new(theater, texts.clone(), true);
        let slots = ["Sort: Newest", "Map: All", "Game type: All", "Lockout"]
            .into_iter()
            .enumerate();

        let string = |id: u32| {
            let values = texts.values.lock().unwrap();
            let bytes = values[(id - TEXTS.base) as usize]
                .strip_suffix(&[0])
                .unwrap();
            std::str::from_utf8(bytes).unwrap().to_owned()
        };
        for (row, label) in slots {
            let cells = rows.texts(row as i16);
            let [(slot, id)] = cells.as_slice() else {
                panic!("the film browser uses one text per row");
            };
            assert_eq!(*slot, crate::ui::LABEL_SLOT);
            assert_eq!(string(*id), label);
        }
        assert_eq!(
            string(rows.focused_footer(Some(0)).unwrap()),
            footer_legend(&library_prompts("Select"))
        );
        assert_eq!(
            string(rows.focused_footer(Some(3)).unwrap()),
            footer_legend(&library_prompts("Details"))
        );
    }

    fn library(films: usize) -> Theater {
        let theater = Theater::default();
        theater.state().load = Load::Ready(
            (0..films)
                .map(|i| FilmMeta {
                    id: format!("film-{i}"),
                    map: if i % 2 == 0 { "05b_deltatowers" } else { "cyclotron" }.into(),
                    started_ms: 1_000 + i as u64,
                    ..FilmMeta::default()
                })
                .collect(),
        );
        theater
    }

    /// The owner's library: 23 films under Sort, Map and Game type. The
    /// list is as tall as its layout and stops at its ends: Down from Sort
    /// walks every row of the real list, the window following the focus,
    /// and neither end wraps.
    #[test]
    fn twenty_three_films_scroll_with_the_focus_and_stop_at_the_ends() {
        use crate::list_class::game_list::{GameList, Step};
        let theater = library(23);
        let rows = ScreenRows::new(theater.clone(), Arc::new(GuestTexts::default()), true);
        assert_eq!(rows.count(), BROWSE_ROWS + 23);
        assert!(!rows.wraps(), "the library stops at its ends");
        assert!(ScreenRows::new(theater, Arc::default(), false).wraps());

        let laid = FILMS_LAYOUT.visible as usize;
        let mut list = GameList::ours(laid, rows.count(), FILMS_LAYOUT.extra_items() as usize, rows.wraps());
        assert_eq!(list.items, laid, "every laid-out row has an item widget");
        list.step(Step::Up);
        assert_eq!(list.pressed(), Some(0), "Up on Sort stays on Sort");
        for want in 1..rows.count() {
            list.step(Step::Down);
            assert_eq!(list.pressed(), Some(want));
            assert!(list.focus < laid);
            assert_eq!(list.first, want.saturating_sub(laid - 1), "the window follows the focus");
        }
        list.step(Step::Down);
        assert_eq!(list.pressed(), Some(rows.count() - 1), "Down on the last film stays");
        assert_eq!(list.arrows(), (true, false));
    }

    /// Sort, Map and Game type each open the picker dialog on A; Left and
    /// Right switch the library's tab, never a filter's value.
    #[test]
    fn the_filters_open_pickers_and_the_arrows_change_tabs() {
        let theater = library(23);
        let rows = ScreenRows::new(theater.clone(), Arc::new(GuestTexts::default()), true);
        for row in 0..BROWSE_ROWS as i16 {
            assert!(
                matches!(rows.press(row), Chosen::Open(crate::ui_kit::picker::NAME)),
                "row {row}"
            );
        }
        let picked = OptionRow::Map.pick(&theater, &theater.state());
        assert_eq!(picked.values, ["All", "Ivory Tower", "Regret"]);
        assert_eq!(rows.with_spec(|s| s.rows[BROWSE_ROWS].label.clone()), "Regret", "newest first, by its level name");
        assert!(matches!(rows.press(BROWSE_ROWS as i16), Chosen::Push(DETAIL)));
        let tab = theater.state().tab;
        rows.sideways(1, true);
        assert_ne!(theater.state().tab, tab);
        assert_eq!(theater.state().selector.map, None, "no filter stepped");
    }

    #[test]
    fn a_wrapped_paragraph_breaks_lines_the_way_the_games_strings_do() {
        let texts = GuestTexts::default();
        let reason = "The film is missing its simulation hook digest.";
        let wrapped = text_fit::paragraph(reason, 120, 8);
        assert!(wrapped.contains('\n'), "{wrapped:?} wraps");
        texts.set(0, &wrapped);
        let stored = texts.values.lock().unwrap()[0].clone();
        let stored = String::from_utf8(stored[..stored.len() - 1].to_vec()).unwrap();
        assert!(!stored.replace("\r\n", "").contains('\n'), "no bare newline, which the game ignores");
        assert_eq!(stored.replace("\r\n", " "), reason, "the break stands in for the space");
        texts.set(1, "one\r\ntwo");
        let stored = texts.values.lock().unwrap()[1].clone();
        assert_eq!(&stored[..stored.len() - 1], b"one\r\ntwo");
    }
}
