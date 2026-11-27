//! Cards, the card row and the carousel (docs/game/ui-screens.md, "Cards
//! and the card row").
//!
//! A card is a framed tile: a name, an art well (a map's picture, or
//! blueprint line-work when it has none), a line of stats and a focus
//! state. A card row lays cards out side by side: the D-pad moves the
//! focus, the row slides with an eased offset when the focus passes its
//! last slot, and the cards past either end peek in, cut at the row's
//! window. A carousel is the row with one slot: the focused card in the
//! middle, its well a map picture's size, its name the screen's one list
//! row, so the game's own list carries the focus and the press.
//!
//! Nothing here is a widget kind of its own. A row lays itself into a
//! [`ScreenSpec`] as what every screen already has: solid fills in
//! [`ScreenSpec::panel`], the map picture in [`ScreenSpec::art`] and the
//! texts in [`ScreenSpec::texts`]. A screen gets its row from its doc's
//! `cards:` block ([`crate::ui_kit::data::CardsDoc`]); [`CardsPage`] is
//! every such screen's page, and [`crate::ui_kit::card_screen`] its class
//! in the game.

use std::sync::Mutex;

use serde::Serialize;

use crate::list_class::ListLayout;
use crate::ui_kit::data::{self, CardsDoc, ScreenDoc};
use crate::ui_kit::input::Input;
use crate::ui_kit::page::Live;
use crate::ui_kit::spec::{DescriptionBlock, ListPlace, PaneText, Press, RowSpec, ScreenSpec, ValueStyle};
use crate::ui_kit::text_fit;
use crate::ui_kit::view::{Art, PaneArt};

/// UI ticks a slide takes (`crate::ui_kit::animation::TICKS_PER_SECOND`
/// is 30, so 200 ms: a screen's close).
pub const SLIDE_TICKS: u8 = 6;

/// The row's window, `left` and `right`: a card is drawn only between
/// them, so a peeking card is cut here and never leaves the canvas.
pub const CLIP: [i16; 2] = [-304, 304];

/// The title-safe area's sides (`crate::ui_kit::layout::SAFE`): a card
/// wholly inside them shows its texts and its picture.
const SAFE: [i16; 2] = [-272, 272];

/// A map picture's size (`crate::mods_screen::page_layout::PICTURE_SIZE`):
/// the game draws a bitmap at its image's size, so a well shows a picture
/// only when it is exactly this big.
pub const PICTURE: [u16; 2] = crate::mods_screen::page_layout::PICTURE_SIZE;

const FILL: [u8; 4] = [12, 23, 39, 238];
const WELL: [u8; 4] = [8, 16, 30, 255];
const FRAME_FOCUSED: [u8; 4] = [173, 195, 216, 255];
const FRAME_REST: [u8; 4] = [99, 148, 193, 150];
const FRAME_PEEK: [u8; 4] = [99, 148, 193, 90];
const GRID: [u8; 4] = [99, 148, 193, 70];

/// The body font (`content/ui/design.yaml`, `fonts.body`).
pub const FONT: i16 = 1;
/// A text line's rectangle height.
pub const LINE: i16 = 20;
/// The card's edge to its well and its texts.
pub const PAD: i16 = 6;
/// A skin-0 list item: its block is 210 wide, from 33 above its anchor to
/// 1 below (`crate::ui_kit::view::Geometry::fallback`).
const ITEM: [i16; 3] = [210, 33, 1];

/// One card's content.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Card {
    pub name: String,
    /// One line under the name: waves, rounds, lives.
    pub stats: String,
    /// The map whose picture fills the well, by its file's stem
    /// (`crate::ui_kit::view::Art::Level`); none draws blueprint lines.
    pub level: Option<String>,
    /// The line under the row while the card has the focus.
    pub help: Option<String>,
    /// The screen A opens on this card; none uses the row's.
    pub open: Option<String>,
}

/// How a row lays its cards out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Layout {
    /// A card's width and height.
    pub size: [i16; 2],
    /// The well's height, under the card's top edge.
    pub well: i16,
    /// The space between two cards.
    pub gap: i16,
    /// Whole cards shown; the cards past either end peek in.
    pub visible: usize,
    /// The cards' top edge.
    pub top: i16,
}

impl Layout {
    /// Four compact cards, blueprint art, each name a text of its card.
    pub const ROW: Layout = Layout {
        size: [124, 150],
        well: 96,
        gap: 8,
        visible: 4,
        top: 130,
    };
    /// One card in the middle: its well the map picture's size, its name
    /// the list's one row under the well, its stats under that.
    pub const CAROUSEL: Layout = Layout {
        size: [PICTURE[0] as i16 + 2 * PAD, 280],
        well: PICTURE[1] as i16,
        gap: 12,
        visible: 1,
        top: 176,
    };

    /// The layout a doc names ([`data::CARD_LAYOUTS`]).
    pub fn named(name: &str) -> Option<Layout> {
        match name {
            "row" => Some(Layout::ROW),
            "carousel" => Some(Layout::CAROUSEL),
            _ => None,
        }
    }

    /// One slot: the focused card's name is the screen's list row.
    pub fn is_carousel(self) -> bool {
        self.visible == 1
    }

    /// One card's step along the row.
    pub fn pitch(self) -> i16 {
        self.size[0] + self.gap
    }

    /// The first slot's left edge: the whole cards are centred.
    pub fn left(self) -> i16 {
        let span = self.visible as i16 * self.pitch() - self.gap;
        -span / 2
    }

    /// The cards' bottom edge.
    pub fn bottom(self) -> i16 {
        self.top - self.size[1]
    }

    /// Whether the well is a map picture's size.
    pub fn holds_picture(self) -> bool {
        [self.size[0] - 2 * PAD, self.well] == [PICTURE[0] as i16, PICTURE[1] as i16]
    }

    /// The well's top-left corner in a card whose left edge is `left`.
    pub fn well_at(self, left: i16) -> [i16; 2] {
        [left + PAD, self.top - PAD]
    }

    /// A carousel's list: one skin-0 row centred under the well.
    pub fn list(self) -> ListLayout {
        ListLayout {
            skin: 0,
            anchor: [-ITEM[0] / 2, self.top - PAD - self.well - 4 - ITEM[1]],
            bitmaps_left_of: i16::MIN,
            visible: 1,
        }
    }

    /// The stats line's rectangle in a card whose left edge is `left`:
    /// the card's last line.
    pub fn stats_rect(self, left: i16) -> [i16; 4] {
        let bottom = self.bottom() + PAD;
        [bottom + LINE, left + PAD, bottom, left + self.size[0] - PAD]
    }

    /// The help block under the cards, and the count beside it.
    pub fn help_rect(self) -> [i16; 4] {
        let top = self.bottom() - 8;
        [top, -260, top - 2 * LINE, 170]
    }
    pub fn count_rect(self) -> [i16; 4] {
        let top = self.bottom() - 8;
        [top, 180, top - LINE, 260]
    }
}

/// Ease out, cubic: fast off the mark, settling into place. `t` from 0
/// to 1 gives 0 to 1.
pub fn ease_out(t: f32) -> f32 {
    let u = 1.0 - t.clamp(0.0, 1.0);
    1.0 - u * u * u
}

/// What a press did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Nothing: the focus is at an end, or the input is not the row's.
    None,
    /// The focus moved to this card.
    Focus(usize),
    /// A on this card: the screen it opens, when it names one.
    Open { card: usize, screen: Option<String> },
    /// B or Back.
    Close,
}

/// A row's state: where the focus is, which card is in the first slot,
/// and the slide still to run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct State {
    pub focus: usize,
    /// The card in the first whole slot.
    pub first: usize,
    /// The offset the slide started from, in UI units: positive draws the
    /// cards right of where they rest.
    pub from: i16,
    /// Ticks into the slide; [`SLIDE_TICKS`] is at rest.
    pub tick: u8,
}

impl State {
    /// At rest with `focus` shown, the row scrolled no further than it
    /// needs.
    pub fn at(focus: usize, cards: usize, layout: Layout) -> State {
        let focus = focus.min(cards.saturating_sub(1));
        State {
            focus,
            first: focus.saturating_sub(layout.visible.saturating_sub(1)),
            from: 0,
            tick: SLIDE_TICKS,
        }
    }

    /// The cards' offset from rest now.
    pub fn offset(self) -> i16 {
        if self.tick >= SLIDE_TICKS {
            return 0;
        }
        let t = f32::from(self.tick) / f32::from(SLIDE_TICKS);
        (f32::from(self.from) * (1.0 - ease_out(t))).round() as i16
    }

    /// Whether a slide is running.
    pub fn sliding(self) -> bool {
        self.offset() != 0
    }

    /// One UI tick of the slide.
    pub fn tick(&mut self) {
        self.tick = self.tick.saturating_add(1).min(SLIDE_TICKS);
    }

    /// Puts the focus on card `focus`, as a list that moved it there
    /// itself reports. The row scrolls when the focus leaves its slots,
    /// and slides from where it is drawn now, so a move during a slide
    /// carries on from it.
    pub fn focus_on(&mut self, focus: usize, cards: usize, layout: Layout) {
        let focus = focus.min(cards.saturating_sub(1));
        let first = if focus < self.first {
            focus
        } else if focus >= self.first + layout.visible {
            focus + 1 - layout.visible
        } else {
            self.first
        };
        if first != self.first {
            let moved = (first as i32 - self.first as i32) * i32::from(layout.pitch());
            let from = i32::from(self.offset()) + moved;
            self.from = from.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
            self.tick = 0;
            self.first = first;
        }
        self.focus = focus;
    }

    /// Moves the focus one card: left and up back, right and down on.
    /// Neither end wraps.
    pub fn step(&mut self, input: Input, cards: usize, layout: Layout) -> Effect {
        let focus = match input {
            Input::Left | Input::Up if self.focus > 0 => self.focus - 1,
            Input::Right | Input::Down if self.focus + 1 < cards => self.focus + 1,
            _ => return Effect::None,
        };
        self.focus_on(focus, cards, layout);
        Effect::Focus(focus)
    }
}

/// A row of cards: the cards, their layout and the screen A opens when a
/// card names none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub layout: Layout,
    pub cards: Vec<Card>,
    pub open: Option<String>,
}

impl Row {
    /// The row a doc's `cards:` block declares: its bound source's cards
    /// when the feature gives two or more ([`bound`]), else the doc's own.
    pub fn of(doc: &CardsDoc) -> Row {
        let cards = doc
            .bind
            .as_deref()
            .and_then(bound)
            .filter(|cards| cards.len() >= 2)
            .unwrap_or_else(|| {
                doc.items
                    .values()
                    .map(|c| Card {
                        name: c.name.clone(),
                        stats: c.stats.clone(),
                        level: c.level.clone(),
                        help: c.help.clone(),
                        open: c.open.clone(),
                    })
                    .collect()
            });
        Row {
            layout: Layout::named(&doc.layout).unwrap_or(Layout::ROW),
            cards,
            open: doc.open.clone(),
        }
    }

    /// The screen A opens on card `i`.
    pub fn opens(&self, i: usize) -> Option<String> {
        self.cards.get(i)?.open.clone().or_else(|| self.open.clone())
    }

    /// One press: the D-pad moves the focus, A opens the focused card, B
    /// and Back close.
    pub fn press(&self, state: &mut State, input: Input) -> Effect {
        match input {
            Input::Left | Input::Right | Input::Up | Input::Down => {
                state.step(input, self.cards.len(), self.layout)
            }
            Input::A if state.focus < self.cards.len() => Effect::Open {
                card: state.focus,
                screen: self.opens(state.focus),
            },
            Input::B | Input::Back => Effect::Close,
            _ => Effect::None,
        }
    }

    /// Card `i`'s left edge in `state`.
    pub fn left_of(&self, state: State, i: usize) -> i16 {
        let slot = i as i32 - state.first as i32;
        (i32::from(self.layout.left()) + slot * i32::from(self.layout.pitch()) + i32::from(state.offset()))
            .clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
    }

    /// The cards drawn in `state`, whole or cut: `(card, left edge)`.
    pub fn shown(&self, state: State) -> Vec<(usize, i16)> {
        (0..self.cards.len())
            .map(|i| (i, self.left_of(state, i)))
            .filter(|(_, left)| *left < CLIP[1] && i32::from(*left) + i32::from(self.layout.size[0]) > i32::from(CLIP[0]))
            .collect()
    }

    /// Lays the row into `spec` in `state`: each shown card's frame, fill,
    /// well and blueprint lines; the map picture and the texts of the
    /// cards wholly inside the safe area; the row's count under them, and
    /// on a row that is no carousel the focused card's help (a carousel's
    /// is its list row's description).
    pub fn place(&self, spec: &mut ScreenSpec, state: State) {
        let (white, pale) = (colour("white", [1.0; 3]), colour("pale_blue", [0.388235, 0.580392, 0.756863]));
        let l = self.layout;
        let [w, _] = l.size;
        for (i, left) in self.shown(state) {
            let card = &self.cards[i];
            let focused = i == state.focus;
            let whole = left >= SAFE[0] && left + w <= SAFE[1];
            let frame = match (focused, whole) {
                (true, _) => FRAME_FOCUSED,
                (false, true) => FRAME_REST,
                (false, false) => FRAME_PEEK,
            };
            let edge = if focused { 2 } else { 1 };
            let (top, bottom, right) = (l.top, l.bottom(), left.saturating_add(w));
            fill(spec, [top, left, bottom, right], frame);
            fill(spec, [top - edge, left.saturating_add(edge), bottom + edge, right - edge], FILL);
            let [well_left, well_top] = l.well_at(left);
            let well = [well_top, well_left, well_top - l.well, right - PAD];
            fill(spec, well, WELL);
            match card.level.as_deref().filter(|_| whole && l.holds_picture()) {
                Some(stem) => spec.art.push(PaneArt {
                    art: Art::Level { stem: stem.to_string() },
                    at: [well_left, well_top],
                    size: PICTURE,
                    depth: 0,
                }),
                None => blueprint(spec, well),
            }
            if !whole {
                continue;
            }
            let room = i32::from(w - 2 * PAD);
            let stats = l.stats_rect(left);
            if !l.is_carousel() {
                spec.texts.push(PaneText {
                    id: format!("card.{i}.name"),
                    rect: [stats[0] + LINE, stats[1], stats[0], stats[3]],
                    font: FONT,
                    colour: if focused { white } else { pale },
                    right: false,
                    text: text_fit::line(&card.name, room),
                });
            }
            if !card.stats.trim().is_empty() {
                spec.texts.push(PaneText {
                    id: format!("card.{i}.stats"),
                    rect: stats,
                    font: FONT,
                    colour: pale,
                    right: false,
                    text: text_fit::line(&card.stats, room),
                });
            }
        }
        if !l.is_carousel()
            && let Some(help) = self.cards.get(state.focus).and_then(|c| c.help.as_deref())
            && !help.trim().is_empty()
        {
            let rect = l.help_rect();
            spec.texts.push(PaneText {
                id: "cards.help".into(),
                rect,
                font: FONT,
                colour: pale,
                right: false,
                text: text_fit::paragraph(help, i32::from(rect[3] - rect[1]) - 10, 2),
            });
        }
        spec.texts.push(PaneText {
            id: "cards.count".into(),
            rect: l.count_rect(),
            font: FONT,
            colour: pale,
            right: true,
            text: count_text(state.focus, self.cards.len()),
        });
    }
}

/// The count beside the help: `2 of 5`.
pub fn count_text(focus: usize, cards: usize) -> String {
    format!("{} of {}", focus + 1, cards)
}

/// A palette colour of the design system, or `or` when it has none.
fn colour(name: &str, or: [f32; 3]) -> [f32; 3] {
    data::active_design().colour(name).unwrap_or(or)
}

/// `rect` (`top, left, bottom, right`) filled, cut to the row's window.
fn fill(spec: &mut ScreenSpec, rect: [i16; 4], rgba: [u8; 4]) {
    let [top, left, bottom, right] = rect;
    let (left, right) = (left.max(CLIP[0]), right.min(CLIP[1]));
    if top > bottom && right > left {
        spec.panel.push(PaneArt {
            art: Art::Solid { rgba },
            at: [left, top],
            size: [(right - left) as u16, (top - bottom) as u16],
            depth: 0,
        });
    }
}

/// Blueprint line-work in a well: its thirds, one unit wide.
fn blueprint(spec: &mut ScreenSpec, well: [i16; 4]) {
    let [top, left, bottom, right] = well;
    let (w, h) = (right - left, top - bottom);
    for k in 1..3 {
        let x = left + w * k / 3;
        fill(spec, [top, x, bottom, x + 1], GRID);
        let y = top - h * k / 3;
        fill(spec, [y, left, y - 1, right], GRID);
    }
}

/// The cards a feature gives a doc's `bind:` name, or `None` for a name
/// no feature answers (the doc's own cards are shown).
pub fn bound(name: &str) -> Option<Vec<Card>> {
    match name {
        "firefight.wavesets" => {
            let map = crate::lobby_start::kit_map();
            let classes = crate::lobby_start::kit_classes();
            Some(crate::firefight::options::wave_cards(
                map.as_deref().or(SAMPLE_MAP),
                classes.as_deref(),
            ))
        }
        _ => None,
    }
}

/// The map the bound sources answer for off a lobby: the one the
/// Firefight page's own tests use.
const SAMPLE_MAP: Option<&str> = Some("beavercreek");

/// The list of a card screen with no list row: it lays out nothing and
/// keeps no frame art.
fn no_list() -> ListLayout {
    ListLayout {
        skin: 0,
        anchor: [-262, 135],
        bitmaps_left_of: i16::MIN,
        visible: 0,
    }
}

/// A card screen's spec from its doc, in `state`.
pub fn spec_of(doc: &ScreenDoc, row: &Row, state: State) -> ScreenSpec {
    let title = doc.text("title", &[]).unwrap_or_default();
    let mut spec = ScreenSpec::new(&doc.screen, &title);
    spec.style = ValueStyle::Inline;
    spec.transparent = doc.backing.as_deref() == Some("none");
    spec.footer = true;
    spec.prompts = doc.prompts("legend").unwrap_or_default();
    if row.layout.is_carousel() {
        spec.list = ListPlace::from(Some(row.layout.list()));
        spec.scrolls = true;
        spec.focus = state.focus.min(row.cards.len().saturating_sub(1));
        spec.inputs = vec![Input::Up, Input::Down, Input::Left, Input::Right, Input::A, Input::B];
        spec.rows = row
            .cards
            .iter()
            .enumerate()
            .map(|(i, card)| {
                let mut r = RowSpec::action(&text_fit::line(&card.name, text_fit::ROW));
                // A card that names no screen is picked: A is the screen's
                // action (a wave set's card picks the set and goes back).
                r = r.press(match row.opens(i) {
                    Some(screen) => Press::Open(screen),
                    None => Press::Act,
                });
                match card.help.as_deref().filter(|h| !h.trim().is_empty()) {
                    Some(help) => r.describe(help),
                    None => r,
                }
            })
            .collect();
        spec.description = Some(DescriptionBlock {
            rect: row.layout.help_rect(),
            font: FONT,
            colour: colour("pale_blue", [0.388235, 0.580392, 0.756863]),
        });
    } else {
        spec.list = ListPlace::from(Some(no_list()));
        spec.inputs = vec![Input::Left, Input::Right, Input::A, Input::B];
        spec.content.card_row = !row.cards.is_empty();
    }
    row.place(&mut spec, state);
    spec
}

/// What a card screen shows now, for `ui.stack`
/// (docs/runtime/agent-api.md, "ui: what the menus show").
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Shown {
    pub screen: String,
    pub layout: Layout,
    pub cards: usize,
    pub focus: usize,
    pub first: usize,
    /// The focused card's name.
    pub name: String,
    /// The slide's offset now, in UI units; 0 at rest.
    pub offset: i16,
}

impl Shown {
    pub fn of(screen: &str, row: &Row, state: State) -> Shown {
        Shown {
            screen: screen.into(),
            layout: row.layout,
            cards: row.cards.len(),
            focus: state.focus,
            first: state.first,
            name: row.cards.get(state.focus).map(|c| c.name.clone()).unwrap_or_default(),
            offset: state.offset(),
        }
    }
}

static SHOWN: Mutex<Option<Shown>> = Mutex::new(None);

/// Says what the card screen on top shows; its class calls this when the
/// state changes, and with `None` when it is freed.
pub fn publish(shown: Option<Shown>) {
    *SHOWN.lock().unwrap_or_else(|e| e.into_inner()) = shown;
}

/// The card screen on top, as last published.
pub fn shown() -> Option<Shown> {
    SHOWN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The page of every screen whose doc has a `cards:` block.
pub struct CardsPage {
    doc: std::sync::Arc<ScreenDoc>,
    row: Row,
    state: State,
}

/// Screen `name`'s page, when its doc declares cards.
pub fn live(name: &str) -> Option<Box<dyn Live>> {
    let doc = data::screen(name)?;
    let row = Row::of(doc.cards.as_ref()?);
    let mut page = CardsPage {
        state: State::at(0, row.cards.len(), row.layout),
        doc,
        row,
    };
    if let Some(first) = page.state_names().first().cloned() {
        page.go(&first);
    }
    Some(Box::new(page))
}

impl CardsPage {
    /// What `ui.stack` would say of this page.
    pub fn shown(&self) -> Shown {
        Shown::of(&self.doc.screen, &self.row, self.state)
    }

    /// A doc state as a row state: at rest on its focus, or `slide`
    /// percent through the slide that brought the focus there from the
    /// card before it.
    fn state_of(&self, focus: usize, slide: Option<u8>) -> State {
        let (cards, layout) = (self.row.cards.len(), self.row.layout);
        let focus = focus.min(cards.saturating_sub(1));
        let Some(percent) = slide.filter(|_| focus > 0) else {
            return State::at(focus, cards, layout);
        };
        let mut state = State::at(focus - 1, cards, layout);
        state.step(Input::Right, cards, layout);
        if state.tick < SLIDE_TICKS {
            state.tick = (u16::from(percent.min(100)) * u16::from(SLIDE_TICKS) / 100) as u8;
        }
        state
    }

    fn named(&self, name: &str) -> Option<State> {
        match self.doc.states.get(name) {
            Some(s) => Some(self.state_of(s.focus, s.slide)),
            None if self.doc.states.is_empty() && name == "first" => {
                Some(State::at(0, self.row.cards.len(), self.row.layout))
            }
            None => None,
        }
    }
}

impl Live for CardsPage {
    fn press(&mut self, input: Input) -> String {
        format!("{:?}", self.row.press(&mut self.state, input))
    }

    fn audit_press(&mut self, input: Input) -> Option<bool> {
        Some(self.row.press(&mut self.state, input) != Effect::None)
    }

    fn spec(&self) -> ScreenSpec {
        spec_of(&self.doc, &self.row, self.state)
    }

    fn summary(&self) -> String {
        let s = self.shown();
        format!(
            "{}: card {} of {} ({:?}), first {}, offset {}",
            s.screen,
            s.focus + 1,
            s.cards,
            s.name,
            s.first,
            s.offset
        )
    }

    fn state_names(&self) -> Vec<String> {
        if self.doc.states.is_empty() {
            return vec!["first".into()];
        }
        self.doc.states.keys().cloned().collect()
    }

    fn specs(&self) -> Vec<(String, ScreenSpec)> {
        self.state_names()
            .into_iter()
            .filter_map(|name| {
                let state = self.named(&name)?;
                Some((name, spec_of(&self.doc, &self.row, state)))
            })
            .collect()
    }

    fn go(&mut self, name: &str) -> bool {
        match self.named(name) {
            Some(state) => {
                self.state = state;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_kit::layout::{Rules, check_bounds};
    use crate::ui_kit::preview::From;

    fn cards(n: usize) -> Vec<Card> {
        (0..n)
            .map(|i| Card {
                name: format!("Card {i}"),
                stats: format!("{} rounds", i + 1),
                level: Some("beavercreek".into()),
                help: Some(format!("What card {i} is.")),
                open: None,
            })
            .collect()
    }

    fn row(layout: Layout, n: usize) -> Row {
        Row {
            layout,
            cards: cards(n),
            open: Some("kit.picker".into()),
        }
    }

    fn placed(row: &Row, state: State) -> ScreenSpec {
        let mut spec = ScreenSpec::new("t", "CARDS");
        row.place(&mut spec, state);
        spec
    }

    #[test]
    fn the_carousel_card_holds_a_map_picture_and_the_row_card_does_not() {
        assert!(Layout::CAROUSEL.holds_picture());
        assert!(!Layout::ROW.holds_picture());
        // The whole cards are centred and inside the safe area.
        for l in [Layout::ROW, Layout::CAROUSEL] {
            let right = l.left() + l.visible as i16 * l.pitch() - l.gap;
            assert_eq!(l.left(), -right, "{l:?}");
            assert!(l.left() >= SAFE[0] && right <= SAFE[1], "{l:?}");
        }
        // The carousel's list row sits under the well and over the stats,
        // inside the card.
        let l = Layout::CAROUSEL;
        let list = l.list();
        let (row_top, row_bottom) = (list.anchor[1] + ITEM[1], list.anchor[1] - ITEM[2]);
        assert!(row_top < l.top - PAD - l.well, "{row_top}");
        assert!(row_bottom >= l.stats_rect(l.left())[0], "{row_bottom}");
        assert!(list.anchor[0] >= l.left() && list.anchor[0] + ITEM[0] <= l.left() + l.size[0]);
    }

    #[test]
    fn the_focus_moves_inside_the_slots_and_the_row_scrolls_past_them() {
        let l = Layout::ROW;
        let mut s = State::at(0, 7, l);
        for want in 1..4 {
            assert_eq!(s.step(Input::Right, 7, l), Effect::Focus(want));
            assert_eq!((s.first, s.sliding()), (0, false), "focus {want}");
        }
        // The fifth card is past the four slots: the row scrolls one card
        // and slides in from a pitch to the right.
        assert_eq!(s.step(Input::Right, 7, l), Effect::Focus(4));
        assert_eq!((s.first, s.offset()), (1, l.pitch()));
        // Left stays inside the slots: no scroll.
        assert_eq!(s.step(Input::Left, 7, l), Effect::Focus(3));
        assert_eq!(s.first, 1);
        // Neither end wraps.
        let mut end = State::at(6, 7, l);
        assert_eq!(end.step(Input::Right, 7, l), Effect::None);
        let mut start = State::at(0, 7, l);
        assert_eq!(start.step(Input::Left, 7, l), Effect::None);
        assert_eq!(start.step(Input::A, 7, l), Effect::None);
    }

    #[test]
    fn the_slide_eases_out_to_rest() {
        let l = Layout::CAROUSEL;
        let mut s = State::at(0, 3, l);
        s.step(Input::Right, 3, l);
        let mut offsets = vec![s.offset()];
        for _ in 0..SLIDE_TICKS {
            s.tick();
            offsets.push(s.offset());
        }
        assert_eq!(offsets[0], l.pitch());
        assert_eq!(*offsets.last().unwrap(), 0);
        assert!(offsets.windows(2).all(|p| p[1] <= p[0]), "{offsets:?}");
        // Eased: more than half the way after a third of the ticks.
        assert!(offsets[2] < l.pitch() / 2, "{offsets:?}");
        // A press during the slide carries on from where the row is drawn.
        let mut s = State::at(0, 3, l);
        s.step(Input::Right, 3, l);
        s.tick();
        let drawn = s.offset();
        s.step(Input::Right, 3, l);
        assert_eq!(s.offset(), drawn + l.pitch());
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
    }

    #[test]
    fn edge_cards_peek_in_cut_to_the_window_with_no_text_or_picture() {
        let r = row(Layout::CAROUSEL, 3);
        let spec = placed(&r, State::at(1, 3, r.layout));
        // Three cards shown: the focused one whole, one either side cut.
        assert_eq!(r.shown(State::at(1, 3, r.layout)).len(), 3);
        for a in &spec.panel {
            assert!(a.at[0] >= CLIP[0] && a.at[0] + a.size[0] as i16 <= CLIP[1], "{a:?}");
        }
        let ids: Vec<&str> = spec.texts.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["card.1.stats", "cards.count"]);
        // One picture, the focused card's, at the picture's own size.
        assert_eq!(spec.art.len(), 1);
        assert_eq!(spec.art[0].size, PICTURE);
        assert_eq!(spec.art[0].art, Art::Level { stem: "beavercreek".into() });
        assert_eq!(spec.texts[1].text, "2 of 3");
        // The first card has no card before it: nothing peeks on the left.
        let first = placed(&r, State::at(0, 3, r.layout));
        assert!(first.panel.iter().all(|a| a.at[0] >= r.layout.left()));
    }

    #[test]
    fn the_focused_card_is_framed_brighter_and_named_in_white() {
        let r = row(Layout::ROW, 6);
        let spec = placed(&r, State::at(2, 6, r.layout));
        let frames: Vec<[u8; 4]> = spec
            .panel
            .iter()
            .filter(|a| a.size[1] == r.layout.size[1] as u16)
            .filter_map(|a| match a.art {
                Art::Solid { rgba } => Some(rgba),
                _ => None,
            })
            .collect();
        assert_eq!(frames.iter().filter(|f| **f == FRAME_FOCUSED).count(), 1);
        assert!(frames.contains(&FRAME_REST) && frames.contains(&FRAME_PEEK), "{frames:?}");
        let name = |i: usize| spec.texts.iter().find(|t| t.id == format!("card.{i}.name")).unwrap();
        assert_eq!(name(2).colour, [1.0; 3]);
        assert_ne!(name(1).colour, [1.0; 3]);
        // The row's cards have no picture: blueprint lines stand in.
        assert!(spec.art.is_empty());
        assert!(spec.texts.iter().any(|t| t.id == "cards.help"));
    }

    #[test]
    fn a_long_name_is_cut_to_its_card() {
        let mut r = row(Layout::ROW, 4);
        r.cards[0].name = "A wave set with a very long name indeed".into();
        let spec = placed(&r, State::at(0, 4, r.layout));
        let room = i32::from(r.layout.size[0] - 2 * PAD);
        assert!(text_fit::width(&spec.texts[0].text) <= room);
        assert!(spec.texts[0].text.ends_with("..."));
    }

    #[test]
    fn a_opens_the_focused_card_and_b_closes() {
        let mut r = row(Layout::ROW, 3);
        r.cards[1].open = Some("forge.variant".into());
        let mut s = State::at(0, 3, r.layout);
        assert_eq!(
            r.press(&mut s, Input::A),
            Effect::Open { card: 0, screen: Some("kit.picker".into()) }
        );
        r.press(&mut s, Input::Right);
        assert_eq!(
            r.press(&mut s, Input::A),
            Effect::Open { card: 1, screen: Some("forge.variant".into()) }
        );
        assert_eq!(r.press(&mut s, Input::B), Effect::Close);
        assert_eq!(r.press(&mut s, Input::X), Effect::None);
    }

    /// A list that moved its own focus reports it: the row follows, and
    /// slides when the card was outside its slots.
    #[test]
    fn a_focus_the_list_moved_is_followed() {
        let l = Layout::CAROUSEL;
        let mut s = State::at(0, 5, l);
        s.focus_on(3, 5, l);
        assert_eq!((s.focus, s.first, s.offset()), (3, 3, 3 * l.pitch()));
        s.focus_on(3, 5, l);
        assert_eq!(s.tick, 0, "the same card again changes nothing");
        s.focus_on(9, 5, l);
        assert_eq!(s.focus, 4);
    }

    /// Every card screen, at rest on every card and through a slide to
    /// it: its texts fit and stay inside the safe area.
    #[test]
    fn every_card_screen_fits_at_every_focus_and_through_every_slide() {
        let source = From::Synthetic;
        let mut seen = 0;
        for name in data::names() {
            let Some(doc) = data::screen(name) else { continue };
            let Some(block) = &doc.cards else { continue };
            seen += 1;
            let row = Row::of(block);
            assert!(row.cards.len() >= 2, "{name}");
            for focus in 0..row.cards.len() {
                let mut state = State::at(focus.saturating_sub(1), row.cards.len(), row.layout);
                if focus > 0 {
                    state.step(Input::Right, row.cards.len(), row.layout);
                }
                for tick in 0..=SLIDE_TICKS {
                    let spec = spec_of(&doc, &row, state);
                    let view = source.view(&spec, "t").unwrap();
                    let issues = check_bounds(&view, source.source(), &Rules::for_spec(&spec));
                    assert!(issues.is_empty(), "{name} focus {focus} tick {tick}: {issues:?}");
                    state.tick();
                }
                assert!(!state.sliding());
            }
        }
        assert!(seen >= 3, "{seen} card screens");
    }

    #[test]
    fn a_page_says_what_ui_stack_would() {
        let page = live("kit.cards").unwrap();
        assert!(page.summary().starts_with("kit.cards: card 1 of "), "{}", page.summary());
    }
}
